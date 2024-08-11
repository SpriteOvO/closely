pub mod bilibili;
pub mod twitter;

use std::{
    fmt::{self, Display},
    future::Future,
    pin::Pin,
    slice, vec,
};

use anyhow::ensure;
use serde::Deserialize;

use crate::{
    config::PlatformGlobal,
    platform::{PlatformMetadata, PlatformTrait},
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "name")]
#[allow(clippy::enum_variant_names)]
pub enum ConfigSourcePlatform {
    #[serde(rename = "bilibili.live")]
    BilibiliLive(bilibili::live::ConfigParams),
    #[serde(rename = "bilibili.space")]
    BilibiliSpace(bilibili::space::ConfigParams),
    #[serde(rename = "bilibili.video")]
    BilibiliVideo(bilibili::video::ConfigParams),
    #[serde(rename = "Twitter")]
    Twitter(twitter::ConfigParams),
}

impl ConfigSourcePlatform {
    pub fn validate(&self, global: &PlatformGlobal) -> anyhow::Result<()> {
        match self {
            Self::BilibiliLive(_) | Self::BilibiliSpace(_) | Self::BilibiliVideo(_) => Ok(()),
            Self::Twitter(p) => p.validate(global),
        }
    }
}

impl fmt::Display for ConfigSourcePlatform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigSourcePlatform::BilibiliLive(p) => write!(f, "{p}"),
            ConfigSourcePlatform::BilibiliSpace(p) => write!(f, "{p}"),
            ConfigSourcePlatform::BilibiliVideo(p) => write!(f, "{p}"),
            ConfigSourcePlatform::Twitter(p) => write!(f, "{p}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct StatusSource {
    pub platform: PlatformMetadata,
    pub user: Option<StatusSourceUser>,
}

#[derive(Clone, Debug)]
pub struct StatusSourceUser {
    pub display_name: String,
    pub profile_url: String,
}

#[derive(Debug)]
pub struct Status(Option<StatusInner>);

#[derive(Debug)]
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

    pub fn is_empty(&self) -> bool {
        self.0.is_none()
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
                        let new_posts = vec_diff_by(&posts.0, &last_posts.0, |l, r| {
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
}

#[derive(Debug)]
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

#[derive(Debug)]
pub struct User {
    pub nickname: String,
    pub profile_url: String,
    pub avatar_url: String,
}

#[derive(Debug, Eq, PartialEq, Hash)]
pub struct PostPlatformUniqueId(String);

#[derive(Debug)]
pub struct Post {
    pub user: Option<User>,
    pub content: String,
    urls: PostUrls,
    pub repost_from: Option<RepostFrom>,
    attachments: Vec<PostAttachment>,
}

impl Post {
    pub fn platform_unique_id(&self) -> PostPlatformUniqueId {
        PostPlatformUniqueId(self.urls.major().unique_id().into())
    }
}

#[derive(Debug)]
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

#[derive(Debug)]
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

#[derive(Debug)]
pub struct PostUrlClickable {
    pub url: String,
    pub display: String,
}

#[derive(Debug)]
pub enum RepostFrom {
    // TODO: Remove this in the future
    Legacy { is_repost: bool, is_quote: bool },
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

#[derive(Debug, Eq, PartialEq)]
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

#[derive(Debug)]
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

#[derive(Debug)]
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

impl<'a> fmt::Display for Notification<'a> {
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

impl<'a> fmt::Display for NotificationKind<'a> {
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
    ) -> Pin<Box<dyn Future<Output = Option<Notification<'a>>> + Send + '_>> {
        Box::pin(async move { Some(notification) })
    }
}

pub fn fetcher(platform: &ConfigSourcePlatform) -> Box<dyn FetcherTrait> {
    match platform {
        ConfigSourcePlatform::BilibiliLive(p) => Box::new(bilibili::live::Fetcher::new(p.clone())),
        ConfigSourcePlatform::BilibiliSpace(p) => {
            Box::new(bilibili::space::Fetcher::new(p.clone()))
        }
        ConfigSourcePlatform::BilibiliVideo(p) => {
            Box::new(bilibili::video::Fetcher::new(p.clone()))
        }
        ConfigSourcePlatform::Twitter(p) => Box::new(twitter::Fetcher::new(p.clone())),
    }
}

fn vec_diff_by<'a, T, F>(lhs: &'a [T], rhs: &'a [T], predicate: F) -> impl Iterator<Item = &'a T>
where
    F: Fn(&T, &T) -> bool + 'a,
{
    lhs.iter()
        .filter(move |l| !rhs.iter().any(|r| predicate(l, r)))
}

#[allow(dead_code)]
fn vec_diff<'a, T>(lhs: &'a [T], rhs: &'a [T]) -> impl Iterator<Item = &'a T>
where
    T: PartialEq,
{
    vec_diff_by(lhs, rhs, |l, r| l == r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec_diff_valid() {
        assert_eq!(
            vec_diff(&[1, 2, 3], &[4, 2, 3, 4]).collect::<Vec<_>>(),
            [&1]
        )
    }
}
