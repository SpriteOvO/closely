mod content;
mod helper;
pub mod platform;

use std::{fmt, fmt::Display, future::Future, pin::Pin, slice, vec};

use anyhow::ensure;
pub use content::*;

use crate::platform::{PlatformMetadata, PlatformTrait};

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
    pub fn attachments_recursive(&self) -> Vec<&PostAttachment> {
        if let Some(RepostFrom::Recursion(repost_from)) = &self.repost_from {
            self.attachments
                .iter()
                .chain(repost_from.attachments_recursive())
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
    Online,
    Offline,
    Banned,
}

impl fmt::Display for LiveStatusKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Online => write!(f, "online"),
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
        if let LiveStatusKind::Online = self.kind {
            write!(f, " with title {}", self.title)?;
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
pub struct Notification<'a> {
    pub kind: NotificationKind<'a>,
    pub source: &'a StatusSource,
}

impl fmt::Display for Notification<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)
    }
}

#[derive(Clone, Debug)]
pub enum NotificationKind<'a> {
    LiveOnline(&'a LiveStatus),
    LiveTitle(&'a LiveStatus, &'a str /* old title */),
    Posts(PostsRef<'a>),
    Log(String),
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
        }
    }
}

pub trait FetcherTrait: PlatformTrait + Display {
    fn fetch_status(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<Status>> + Send + '_>>;

    // "post" means "after" here
    fn post_filter<'a>(
        &'a self,
        notification: Notification<'a>,
    ) -> Pin<Box<dyn Future<Output = Option<Notification<'a>>> + Send + 'a>> {
        Box::pin(async move { Some(notification) })
    }
}

pub fn fetcher(platform: &platform::Config) -> Box<dyn FetcherTrait> {
    match platform {
        platform::Config::BilibiliLive(p) => {
            Box::new(platform::bilibili::live::Fetcher::new(p.clone()))
        }
        platform::Config::BilibiliSpace(p) => {
            Box::new(platform::bilibili::space::Fetcher::new(p.clone()))
        }
        platform::Config::BilibiliVideo(p) => {
            Box::new(platform::bilibili::video::Fetcher::new(p.clone()))
        }
        platform::Config::Twitter(p) => Box::new(platform::twitter::Fetcher::new(p.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_incremental_update_live() {
        let mut status = Status::empty();
        assert!(status.0.is_none());

        let new = Status::new(
            StatusKind::Live(LiveStatus {
                kind: LiveStatusKind::Online,
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
                kind: LiveStatusKind::Online,
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
                    repost_from: None,
                    attachments: vec![],
                },
                Post {
                    user: None,
                    content: PostContent::plain("content2"),
                    urls: PostUrls::new(PostUrl::Identity("id2".into())),
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
                    repost_from: None,
                    attachments: vec![],
                },
                Post {
                    user: None,
                    content: PostContent::plain("content2"),
                    urls: PostUrls::new(PostUrl::Identity("id2".into())),
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
                    repost_from: None,
                    attachments: vec![],
                },
                Post {
                    user: None,
                    content: PostContent::plain("content2"),
                    urls: PostUrls::new(PostUrl::Identity("id2".into())),
                    repost_from: None,
                    attachments: vec![],
                },
                Post {
                    user: None,
                    content: PostContent::plain("content3"),
                    urls: PostUrls::new(PostUrl::Identity("id3".into())),
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
                kind: LiveStatusKind::Online,
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
}
