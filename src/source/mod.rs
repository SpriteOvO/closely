mod content;
mod helper;
pub mod platform;

use std::{
    collections::{hash_map::Entry, HashMap},
    fmt::{self, Display},
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    slice,
    time::SystemTime,
    vec,
};

use anyhow::{anyhow, ensure};
use bytes::Bytes;
use chrono::{DateTime, Local};
pub use content::*;
use humantime_serde::re::humantime;
use spdlog::prelude::*;
use tempfile::tempdir;
use tokio::{
    fs,
    sync::{mpsc, Mutex},
};

use crate::{
    config,
    helper::VideoResolution,
    platform::{PlatformMetadata, PlatformTrait},
};

#[derive(Clone, Debug, PartialEq)]
pub struct StatusSource {
    pub platform: PlatformMetadata,
    pub user: Option<StatusSourceUser>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatusSourceUser {
    pub display_name: String,
    pub profile_url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Status(Option<StatusInner>);

#[derive(Clone, Debug, PartialEq)]
struct StatusInner {
    kind: StatusKind,
    source: StatusSource,
}

impl Status {
    pub fn empty() -> Self {
        Self(None)
    }

    pub fn new(kind: StatusKind, source: StatusSource) -> Self {
        Self(Some(StatusInner { kind, source }))
    }

    pub fn sort(&mut self) {
        if let Some(StatusInner {
            kind: StatusKind::Posts(posts),
            ..
        }) = &mut self.0
        {
            // Latest first
            posts.0.sort_by(|l, r| r.time.cmp(&l.time));
        }
    }

    pub fn generate_notifications<'a>(&'a self, last_status: &'a Status) -> Vec<Notification<'a>> {
        self.0
            .as_ref()
            .map(
                |status| match (&status.kind, last_status.0.as_ref().map(|s| &s.kind)) {
                    (StatusKind::Live(live_status), Some(StatusKind::Live(last_live_status))) => {
                        let mut notifications = vec![];
                        if live_status.title != last_live_status.title {
                            notifications.push(Notification {
                                kind: NotificationKind::LiveTitle(
                                    live_status,
                                    &last_live_status.title,
                                ),
                                source: &status.source,
                            })
                        }
                        if live_status.kind != last_live_status.kind {
                            notifications.push(Notification {
                                kind: NotificationKind::LiveOnline(live_status),
                                source: &status.source,
                            })
                        }
                        notifications
                    }
                    (StatusKind::Posts(posts), Some(StatusKind::Posts(last_posts))) => {
                        let new_posts = helper::diff_by(&last_posts.0, &posts.0, |l, r| {
                            l.platform_unique_id() == r.platform_unique_id()
                        })
                        .collect::<Vec<_>>();
                        if !new_posts.is_empty() {
                            vec![Notification {
                                kind: NotificationKind::Posts(PostsRef(new_posts)),
                                source: &status.source,
                            }]
                        } else {
                            vec![]
                        }
                    }
                    (_, None) => vec![],
                    (_, _) => panic!("states mismatch"),
                },
            )
            .unwrap_or_default()
    }

    // Sometimes the data source API glitches and returns empty items without
    // producing any errors. If we simply replace the stored value of `Status`, when
    // the API comes back to normal, we will incorrectly generate notifications with
    // all the items as a new update. To solve this issue, call this function, which
    // will always incrementally store the new items and never delete the old items.
    pub fn update_incrementally(&mut self, new: Status) {
        match (&mut self.0, new.0) {
            (None, None) => {}
            (Some(_), None) => {}
            (inner @ None, Some(new)) => *inner = Some(new),
            (Some(stored), Some(new)) => {
                match (&mut stored.kind, new.kind) {
                    (StatusKind::Live(stored), StatusKind::Live(new)) => *stored = new,
                    (StatusKind::Posts(stored), StatusKind::Posts(new)) => {
                        let mut new = helper::diff_by(&stored.0, new.0, |l, r| {
                            l.platform_unique_id() == r.platform_unique_id()
                        })
                        .collect::<Vec<_>>();
                        // We don't care about the order at the moment.
                        stored.0.append(&mut new);
                    }
                    _ => unreachable!("the stored status and the new status kinds are mismatch"),
                }
                stored.source.platform = new.source.platform;
                if let Some(user) = new.source.user {
                    stored.source.user = Some(user);
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StatusKind {
    Live(LiveStatus),
    Posts(Posts),
}

impl fmt::Display for StatusKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Live(live_status) => write!(f, "{}", live_status),
            Self::Posts(posts) => write!(f, "{}", posts),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Update {
    kind: UpdateKind,
    source: StatusSource, // TODO: rename the type
}

impl Update {
    pub fn new(kind: UpdateKind, source: StatusSource) -> Self {
        Self { kind, source }
    }

    pub async fn generate_notifications(&self) -> Vec<Notification<'_>> {
        match &self.kind {
            UpdateKind::Playback(playback) => {
                vec![Notification {
                    kind: NotificationKind::Playback(PlaybackRef {
                        live_start_time: playback.live_start_time,
                        local_file: (&playback.file_path, playback.format),
                        loaded: Mutex::new(HashMap::new()),
                    }),
                    source: &self.source,
                }]
            }
            UpdateKind::Document(document) => {
                let Ok(file) = FileRef::new(&document.file_path).await.inspect_err(|err| {
                    error!(
                        "failed to read document file '{:?}': {err}",
                        document.file_path
                    )
                }) else {
                    return vec![];
                };

                vec![Notification {
                    kind: NotificationKind::Document(DocumentRef { file }),
                    source: &self.source,
                }]
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateKind {
    Playback(Playback),
    Document(Document),
}

impl fmt::Display for UpdateKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Playback(playback) => write!(f, "{playback}"),
            Self::Document(document) => write!(f, "{document}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct User {
    pub nickname: String,
    pub profile_url: String,
    pub avatar_url: String,
}

#[derive(Debug, Eq, PartialEq, Hash)]
pub struct PostPlatformUniqueId(String);

#[derive(Clone, Debug, PartialEq)]
pub struct Post {
    pub user: Option<User>,
    pub content: PostContent,
    urls: PostUrls,
    pub time: DateTime<Local>,
    pub is_pinned: bool,
    pub repost_from: Option<RepostFrom>,
    attachments: Vec<PostAttachment>,
}

impl Post {
    pub fn platform_unique_id(&self) -> PostPlatformUniqueId {
        PostPlatformUniqueId(self.urls.major().unique_id().into())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PostUrls(Vec<PostUrl>);

impl PostUrls {
    pub fn new(url: PostUrl) -> Self {
        Self(vec![url])
    }

    pub fn from_iter(urls: impl IntoIterator<Item = PostUrl>) -> anyhow::Result<Self> {
        let urls = urls.into_iter().collect::<Vec<_>>();
        ensure!(!urls.is_empty(), "urls must not be empty");
        Ok(Self(urls))
    }

    pub fn major(&self) -> &PostUrl {
        self.0.first().unwrap()
    }

    pub fn iter(&self) -> slice::Iter<PostUrl> {
        self.0.iter()
    }
}

impl From<PostUrl> for PostUrls {
    fn from(url: PostUrl) -> Self {
        Self::new(url)
    }
}

#[derive(Debug)]
pub struct PostUrlsRef<'a>(Vec<&'a PostUrl>);

impl<'a> PostUrlsRef<'a> {
    pub fn major(&self) -> &'a PostUrl {
        self.0.first().unwrap()
    }

    pub fn iter(&self) -> slice::Iter<'_, &'a PostUrl> {
        self.0.iter()
    }

    pub fn into_iter(self) -> vec::IntoIter<&'a PostUrl> {
        self.0.into_iter()
    }
}

#[derive(Clone, Debug)]
pub enum PostUrl {
    Clickable(PostUrlClickable),
    // For some cases. a post doesn't have a URL (e.g. deleted post), but we still need something
    // unique to identify it
    Identity(String),
}

impl PostUrl {
    pub fn new_clickable(url: impl Into<String>, display: impl Into<String>) -> Self {
        PostUrl::Clickable(PostUrlClickable {
            url: url.into(),
            display: display.into(),
        })
    }

    pub fn as_clickable(&self) -> Option<&PostUrlClickable> {
        match self {
            PostUrl::Clickable(clickable) => Some(clickable),
            _ => None,
        }
    }

    pub fn unique_id(&self) -> &str {
        let id = match self {
            PostUrl::Clickable(clickable) => &clickable.url,
            PostUrl::Identity(identity) => identity,
        };
        assert!(!id.is_empty());
        id
    }
}

impl PartialEq for PostUrl {
    fn eq(&self, other: &Self) -> bool {
        self.unique_id().eq(other.unique_id())
    }
}

#[derive(Clone, Debug)]
pub struct PostUrlClickable {
    pub url: String,
    pub display: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RepostFrom {
    Recursion(Box<Post>),
}

impl Post {
    pub fn attachments_recursive(&self, include_inlined: bool) -> Vec<&PostAttachment> {
        if let Some(RepostFrom::Recursion(repost_from)) = &self.repost_from {
            self.attachments
                .iter()
                .chain(self.content.parts().filter_map(|content| {
                    if !include_inlined {
                        return None;
                    }
                    if let PostContentPart::InlineAttachment(attachment) = content {
                        Some(attachment)
                    } else {
                        None
                    }
                }))
                .chain(repost_from.attachments_recursive(include_inlined))
                .collect()
        } else {
            self.attachments.iter().collect()
        }
    }

    pub fn urls_recursive(&self) -> PostUrlsRef {
        if let Some(RepostFrom::Recursion(repost_from)) = &self.repost_from {
            let mut v = self
                .urls
                .iter()
                .chain(repost_from.urls_recursive().into_iter().skip(1)) // Skip the major URL from the repost
                .collect::<Vec<_>>();
            v.dedup_by_key(|url| url.unique_id());
            PostUrlsRef(v)
        } else {
            PostUrlsRef(self.urls.iter().collect())
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PostAttachment {
    Image(PostAttachmentImage),
    Video(PostAttachmentVideo),
}

#[derive(Clone, Debug)]
pub struct PostAttachmentImage {
    pub media_url: String,
    pub has_spoiler: bool,
}

impl PartialEq for PostAttachmentImage {
    fn eq(&self, other: &Self) -> bool {
        self.media_url.eq(&other.media_url)
    }
}

#[derive(Clone, Debug)]
pub struct PostAttachmentVideo {
    pub media_url: String,
    pub has_spoiler: bool,
}

impl PartialEq for PostAttachmentVideo {
    fn eq(&self, other: &Self) -> bool {
        self.media_url.eq(&other.media_url)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LiveStatusKind {
    Online { start_time: Option<SystemTime> },
    Offline,
    Banned,
}

impl fmt::Display for LiveStatusKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Online { start_time: _ } => write!(f, "online"),
            Self::Offline => write!(f, "offline"),
            Self::Banned => write!(f, "banned"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LiveStatus {
    pub kind: LiveStatusKind,
    pub title: String,
    pub streamer_name: String,
    pub cover_image_url: String,
    pub live_url: String,
}

impl fmt::Display for LiveStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "'{}' {}", self.streamer_name, self.kind)?;
        if let LiveStatusKind::Online { start_time } = self.kind {
            write!(
                f,
                " with title '{}' started at {:?}",
                self.title,
                start_time.map(humantime::format_rfc3339)
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Posts(Vec<Post>);

impl fmt::Display for Posts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?}",
            self.0.iter().map(|p| p.urls.major()).collect::<Vec<_>>()
        )
    }
}

#[derive(Clone, Debug)]
pub struct PostsRef<'a>(pub Vec<&'a Post>);

impl fmt::Display for PostsRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?}",
            self.0.iter().map(|p| p.urls.major()).collect::<Vec<_>>()
        )
    }
}

#[derive(Clone, Debug)]
pub struct FileRef<'a> {
    pub path: Option<&'a Path>,
    pub name: String,
    pub data: Bytes,
    pub size: u64,
}

impl<'a> FileRef<'a> {
    pub async fn new(path: &'a Path) -> anyhow::Result<Self> {
        let mut ret = Self::read_to_mem(path).await?;
        ret.path = Some(path);
        Ok(ret)
    }

    pub async fn read_to_mem(path: &Path) -> anyhow::Result<Self> {
        let metadata = fs::metadata(path)
            .await
            .map_err(|err| anyhow!("failed to get file size of file '{path:?}': {err}"))?;

        let file_type = metadata.file_type();
        ensure!(
            file_type.is_file() && !file_type.is_symlink(),
            "file '{path:?}' is not a regular file"
        );

        let data = fs::read(path)
            .await
            .map_err(|err| anyhow!("failed to read file '{path:?}': {err}"))?;

        Ok(Self {
            path: None,
            name: path
                .file_name()
                .ok_or_else(|| anyhow!("failed to get file name of file '{path:?}'"))?
                .to_string_lossy()
                .into(),
            data: data.into(),
            size: metadata.len(),
        })
    }
}

impl fmt::Display for FileRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "'{}' ({:?})", self.name, self.size)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Playback {
    pub live_start_time: Option<DateTime<Local>>,
    pub file_path: PathBuf,
    pub format: PlaybackFormat,
}

impl fmt::Display for Playback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "playback file '{}' started at {:?}",
            self.file_path.display(),
            self.live_start_time
                .map(|t| t.to_string())
                .unwrap_or_else(|| "unknown".into()),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlaybackFormat {
    Flv,
    Mp4,
}

impl PlaybackFormat {
    pub fn extension(&self) -> &str {
        match self {
            Self::Flv => "flv",
            Self::Mp4 => "mp4",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlaybackLoaded<'a> {
    pub file: FileRef<'a>,
    pub resolution: VideoResolution,
}

#[derive(Debug)]
pub struct PlaybackRef<'a> {
    pub live_start_time: Option<DateTime<Local>>,
    local_file: (&'a Path, PlaybackFormat),
    pub loaded: Mutex<HashMap<PlaybackFormat, PlaybackLoaded<'a>>>,
}

impl PlaybackRef<'_> {
    pub async fn get(&self, format: PlaybackFormat) -> anyhow::Result<PlaybackLoaded<'_>> {
        let mut loaded = self.loaded.lock().await;
        match loaded.entry(format) {
            Entry::Occupied(entry) => Ok(entry.get().clone()),
            Entry::Vacant(entry) => {
                if self.local_file.1 == format {
                    let file = FileRef::read_to_mem(self.local_file.0).await?;
                    let resolution = crate::helper::ffprobe_resolution(self.local_file.0).await?;
                    let loaded = PlaybackLoaded { file, resolution };
                    entry.insert(loaded.clone());
                    Ok(loaded)
                } else {
                    let dir =
                        tempdir().map_err(|err| anyhow!("failed to create temp dir: {err}"))?;
                    let src = self.local_file.0;
                    let target = dir
                        .path()
                        .join(src.file_name().unwrap_or_else(|| "unknown".as_ref()))
                        .with_extension(format.extension());

                    trace!("converting playback file from '{src:?}' to '{target:?}'");
                    crate::helper::ffmpeg_copy(src, &target).await?;
                    trace!("converting done.");

                    let mut converted = FileRef::read_to_mem(&target).await?;
                    converted.path = None; // The temp file will be deleted when dropped
                    let resolution = crate::helper::ffprobe_resolution(&target).await?;
                    let loaded = PlaybackLoaded {
                        file: converted,
                        resolution,
                    };
                    entry.insert(loaded.clone());
                    Ok(loaded)
                }
            }
        }
    }
}

impl fmt::Display for PlaybackRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "playback file {:?} started at {:?}",
            self.local_file.0,
            self.live_start_time
                .map(|t| t.to_string())
                .unwrap_or_else(|| "unknown".into()),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub file_path: PathBuf,
}

impl fmt::Display for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "document file '{}' started", self.file_path.display())
    }
}

#[derive(Clone, Debug)]
pub struct DocumentRef<'a> {
    pub file: FileRef<'a>,
}

impl fmt::Display for DocumentRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "document file {}", self.file)
    }
}

#[derive(Debug)]
pub struct Notification<'a> {
    pub kind: NotificationKind<'a>,
    pub source: &'a StatusSource,
}

impl fmt::Display for Notification<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)
    }
}

#[derive(Debug)]
pub enum NotificationKind<'a> {
    LiveOnline(&'a LiveStatus),
    LiveTitle(&'a LiveStatus, &'a str /* old title */),
    Posts(PostsRef<'a>),
    Log(String),
    Playback(PlaybackRef<'a>),
    Document(DocumentRef<'a>),
}

impl fmt::Display for NotificationKind<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LiveOnline(live_status) => write!(f, "{live_status}"),
            Self::LiveTitle(live_status, old_title) => {
                write!(f, "{live_status}, old title '{old_title}'")
            }
            Self::Posts(posts) => write!(f, "{}", posts),
            Self::Log(message) => write!(f, "log '{message}'"),
            Self::Playback(playback) => write!(f, "{playback}"),
            Self::Document(document) => write!(f, "{document}"),
        }
    }
}

pub trait FetcherTrait: PlatformTrait + Display {
    fn fetch_status(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<Status>> + Send + '_>>;
}

pub trait ListenerTrait: PlatformTrait + Display {
    fn listen(
        &mut self,
        sender: mpsc::Sender<Update>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

pub enum Sourcer {
    // Poll-based
    Fetcher(Box<dyn FetcherTrait>),

    // Listen-based
    Listener(Box<dyn ListenerTrait>),
}

impl Sourcer {
    fn new_fetcher(f: impl FetcherTrait + 'static) -> Self {
        Self::Fetcher(Box::new(f))
    }

    fn new_listener(l: impl ListenerTrait + 'static) -> Self {
        Self::Listener(Box::new(l))
    }
}

pub fn sourcer(platform: &config::Accessor<platform::Config>) -> Sourcer {
    match &**platform {
        platform::Config::BilibiliLive(p) => {
            Sourcer::new_fetcher(platform::bilibili::live::Fetcher::new(p.clone()))
        }
        platform::Config::BilibiliSpace(p) => {
            Sourcer::new_fetcher(platform::bilibili::space::Fetcher::new(p.clone()))
        }
        platform::Config::BilibiliVideo(p) => {
            Sourcer::new_fetcher(platform::bilibili::video::Fetcher::new(p.clone()))
        }
        platform::Config::BilibiliPlayback(p) => {
            Sourcer::new_listener(platform::bilibili::playback::Listener::new(p.clone()))
        }
        platform::Config::Twitter(p) => {
            Sourcer::new_fetcher(platform::twitter::Fetcher::new(p.clone()))
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Days;

    use super::*;

    #[test]
    fn status_incremental_update_live() {
        let mut status = Status::empty();
        assert!(status.0.is_none());

        let new = Status::new(
            StatusKind::Live(LiveStatus {
                kind: LiveStatusKind::Online { start_time: None },
                title: "title1".into(),
                streamer_name: "streamer1".into(),
                cover_image_url: "cover1".into(),
                live_url: "live1".into(),
            }),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: Some(StatusSourceUser {
                    display_name: "user1".into(),
                    profile_url: "profile1".into(),
                }),
            },
        );
        status.update_incrementally(new.clone());
        assert_eq!(status, new);

        let new = Status::new(
            StatusKind::Live(LiveStatus {
                kind: LiveStatusKind::Online { start_time: None },
                title: "title2".into(),
                streamer_name: "streamer2".into(),
                cover_image_url: "cover2".into(),
                live_url: "live2".into(),
            }),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: Some(StatusSourceUser {
                    display_name: "user1".into(),
                    profile_url: "profile1".into(),
                }),
            },
        );
        status.update_incrementally(new.clone());
        assert_eq!(status, new);
    }

    #[test]
    fn status_incremental_update_posts() {
        let mut status = Status::empty();
        assert!(status.0.is_none());

        let new = Status::new(
            StatusKind::Posts(Posts(vec![Post {
                user: None,
                content: PostContent::plain("content1"),
                urls: PostUrls::new(PostUrl::Identity("id1".into())),
                time: DateTime::UNIX_EPOCH.into(),
                is_pinned: false,
                repost_from: None,
                attachments: vec![],
            }])),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: Some(StatusSourceUser {
                    display_name: "user1".into(),
                    profile_url: "profile1".into(),
                }),
            },
        );
        status.update_incrementally(new.clone());
        assert_eq!(status, new);

        status.update_incrementally(Status::new(
            StatusKind::Posts(Posts(vec![
                Post {
                    user: None,
                    content: PostContent::plain("content1"),
                    urls: PostUrls::new(PostUrl::Identity("id1".into())),
                    time: DateTime::UNIX_EPOCH.into(),
                    is_pinned: false,
                    repost_from: None,
                    attachments: vec![],
                },
                Post {
                    user: None,
                    content: PostContent::plain("content2"),
                    urls: PostUrls::new(PostUrl::Identity("id2".into())),
                    time: DateTime::UNIX_EPOCH.into(),
                    is_pinned: false,
                    repost_from: None,
                    attachments: vec![],
                },
            ])),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: None,
            },
        ));
        let last = Status::new(
            StatusKind::Posts(Posts(vec![
                Post {
                    user: None,
                    content: PostContent::plain("content1"),
                    urls: PostUrls::new(PostUrl::Identity("id1".into())),
                    time: DateTime::UNIX_EPOCH.into(),
                    is_pinned: false,
                    repost_from: None,
                    attachments: vec![],
                },
                Post {
                    user: None,
                    content: PostContent::plain("content2"),
                    urls: PostUrls::new(PostUrl::Identity("id2".into())),
                    time: DateTime::UNIX_EPOCH.into(),
                    is_pinned: false,
                    repost_from: None,
                    attachments: vec![],
                },
            ])),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: Some(StatusSourceUser {
                    display_name: "user1".into(),
                    profile_url: "profile1".into(),
                }),
            },
        );
        assert_eq!(status, last);

        status.update_incrementally(Status::new(
            StatusKind::Posts(Posts(vec![])),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: None,
            },
        ));
        assert_eq!(status, last);

        status.update_incrementally(Status::new(
            StatusKind::Posts(Posts(vec![Post {
                user: None,
                content: PostContent::plain("content3"),
                urls: PostUrls::new(PostUrl::Identity("id3".into())),
                time: DateTime::UNIX_EPOCH.into(),
                is_pinned: false,
                repost_from: None,
                attachments: vec![],
            }])),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: Some(StatusSourceUser {
                    display_name: "user2".into(),
                    profile_url: "profile1".into(),
                }),
            },
        ));
        let last = Status::new(
            StatusKind::Posts(Posts(vec![
                Post {
                    user: None,
                    content: PostContent::plain("content1"),
                    urls: PostUrls::new(PostUrl::Identity("id1".into())),
                    time: DateTime::UNIX_EPOCH.into(),
                    is_pinned: false,
                    repost_from: None,
                    attachments: vec![],
                },
                Post {
                    user: None,
                    content: PostContent::plain("content2"),
                    urls: PostUrls::new(PostUrl::Identity("id2".into())),
                    time: DateTime::UNIX_EPOCH.into(),
                    is_pinned: false,
                    repost_from: None,
                    attachments: vec![],
                },
                Post {
                    user: None,
                    content: PostContent::plain("content3"),
                    urls: PostUrls::new(PostUrl::Identity("id3".into())),
                    time: DateTime::UNIX_EPOCH.into(),
                    is_pinned: false,
                    repost_from: None,
                    attachments: vec![],
                },
            ])),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: Some(StatusSourceUser {
                    display_name: "user2".into(),
                    profile_url: "profile1".into(),
                }),
            },
        );
        assert_eq!(status, last);

        status.update_incrementally(Status::empty());
        assert_eq!(status, last);
    }

    #[test]
    #[should_panic]
    fn status_incremental_update_mismatch() {
        let mut status = Status::empty();
        assert!(status.0.is_none());

        let new = Status::new(
            StatusKind::Live(LiveStatus {
                kind: LiveStatusKind::Online { start_time: None },
                title: "title1".into(),
                streamer_name: "streamer1".into(),
                cover_image_url: "cover1".into(),
                live_url: "live1".into(),
            }),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: Some(StatusSourceUser {
                    display_name: "user1".into(),
                    profile_url: "profile1".into(),
                }),
            },
        );
        status.update_incrementally(new.clone());
        assert_eq!(status, new);

        status.update_incrementally(Status::new(
            StatusKind::Posts(Posts(vec![Post {
                user: None,
                content: PostContent::plain("content1"),
                urls: PostUrls::new(PostUrl::Identity("id1".into())),
                time: DateTime::UNIX_EPOCH.into(),
                is_pinned: false,
                repost_from: None,
                attachments: vec![],
            }])),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: Some(StatusSourceUser {
                    display_name: "user1".into(),
                    profile_url: "profile1".into(),
                }),
            },
        ));
    }

    #[test]
    fn status_posts_sort() {
        let mut status = Status::new(
            StatusKind::Posts(Posts(vec![
                Post {
                    user: None,
                    content: PostContent::plain("content2"),
                    urls: PostUrls::new(PostUrl::Identity("id2".into())),
                    time: DateTime::UNIX_EPOCH
                        .checked_add_days(Days::new(1))
                        .unwrap()
                        .into(),
                    is_pinned: true,
                    repost_from: None,
                    attachments: vec![],
                },
                Post {
                    user: None,
                    content: PostContent::plain("content3"),
                    urls: PostUrls::new(PostUrl::Identity("id3".into())),
                    time: DateTime::UNIX_EPOCH
                        .checked_add_days(Days::new(2))
                        .unwrap()
                        .into(),
                    is_pinned: false,
                    repost_from: None,
                    attachments: vec![],
                },
                Post {
                    user: None,
                    content: PostContent::plain("content1"),
                    urls: PostUrls::new(PostUrl::Identity("id1".into())),
                    time: DateTime::UNIX_EPOCH.into(),
                    is_pinned: false,
                    repost_from: None,
                    attachments: vec![],
                },
            ])),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: Some(StatusSourceUser {
                    display_name: "user2".into(),
                    profile_url: "profile1".into(),
                }),
            },
        );

        status.sort();

        let StatusKind::Posts(posts) = status.0.unwrap().kind else {
            panic!()
        };
        assert_eq!(posts.0[0].content.fallback(), "content3");
        assert_eq!(posts.0[1].content.fallback(), "content2");
        assert_eq!(posts.0[2].content.fallback(), "content1");
    }
}
