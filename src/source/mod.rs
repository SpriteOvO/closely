pub mod bilibili;
pub mod twitter;

use std::{
    fmt::{self, Display},
    future::Future,
    pin::Pin,
};

use anyhow::ensure;
use serde::Deserialize;

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

#[derive(Debug)]
#[allow(clippy::enum_variant_names)]
pub enum SourcePlatformName {
    BilibiliLive,
    BilibiliSpace,
    BilibiliVideo,
    Twitter,
}

impl fmt::Display for SourcePlatformName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BilibiliLive => write!(f, "bilibili 直播"),
            Self::BilibiliSpace => write!(f, "bilibili 动态"),
            Self::BilibiliVideo => write!(f, "bilibili 视频"),
            Self::Twitter => write!(f, "Twitter"),
        }
    }
}

#[derive(Debug)]
pub struct StatusSource {
    pub platform_name: SourcePlatformName,
    pub user: Option<StatusSourceUser>,
}

#[derive(Debug)]
pub struct StatusSourceUser {
    pub display_name: String,
    pub profile_url: String,
}

#[derive(Debug)]
pub struct Status {
    pub kind: StatusKind,
    pub source: StatusSource,
}

impl Status {
    pub fn generate_notifications<'a>(
        &'a self,
        last_status: Option<&'a Status>,
    ) -> Vec<Notification<'a>> {
        match (&self.kind, last_status.map(|s| &s.kind)) {
            (StatusKind::Live(live_status), Some(StatusKind::Live(last_live_status))) => {
                let mut notifications = vec![];
                if live_status.title != last_live_status.title {
                    notifications.push(Notification {
                        kind: NotificationKind::LiveTitle(live_status, &last_live_status.title),
                        source: &self.source,
                    })
                }
                if live_status.online != last_live_status.online {
                    notifications.push(Notification {
                        kind: NotificationKind::LiveOnline(live_status),
                        source: &self.source,
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
                        source: &self.source,
                    }]
                } else {
                    vec![]
                }
            }
            (_, None) => vec![],
            (_, _) => panic!("states mismatch"),
        }
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
    pub urls: PostUrls,
    pub repost_from: Option<RepostFrom>,
    attachments: Vec<PostAttachment>,
}

impl Post {
    pub fn platform_unique_id(&self) -> PostPlatformUniqueId {
        PostPlatformUniqueId(self.urls.major().url.clone())
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

    pub fn iter(&self) -> impl IntoIterator<Item = &PostUrl> {
        self.0.iter()
    }
}

impl From<PostUrl> for PostUrls {
    fn from(url: PostUrl) -> Self {
        Self::new(url)
    }
}

#[derive(Debug)]
pub struct PostUrl {
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
}

#[derive(Clone, Debug)]
pub enum PostAttachment {
    Image(PostAttachmentImage),
    #[allow(dead_code)]
    Video(PostAttachmentVideo),
}

#[derive(Clone, Debug)]
pub struct PostAttachmentImage {
    pub media_url: String,
}

#[derive(Clone, Debug)]
pub struct PostAttachmentVideo {
    pub media_url: String,
}

#[derive(Debug)]
pub struct LiveStatus {
    pub online: bool,
    pub title: String,
    pub streamer_name: String,
    pub cover_image_url: String,
    pub live_url: String,
}

impl fmt::Display for LiveStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "'{}' {}",
            self.streamer_name,
            if self.online { "online" } else { "offline" }
        )?;
        if self.online {
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

#[derive(Debug)]
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

#[derive(Debug)]
pub struct Notification<'a> {
    pub kind: NotificationKind<'a>,
    pub source: &'a StatusSource,
}

impl<'a> fmt::Display for Notification<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)
    }
}

#[derive(Debug)]
pub enum NotificationKind<'a> {
    LiveOnline(&'a LiveStatus),
    LiveTitle(&'a LiveStatus, &'a str /* old title */),
    Posts(PostsRef<'a>),
}

impl<'a> fmt::Display for NotificationKind<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LiveOnline(live_status) => write!(f, "{live_status}"),
            Self::LiveTitle(live_status, old_title) => {
                write!(f, "{live_status}, old title '{old_title}'")
            }
            Self::Posts(posts) => write!(f, "{}", posts),
        }
    }
}

pub trait FetcherTrait: Display + Send + Sync {
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
