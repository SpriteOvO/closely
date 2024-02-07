pub(crate) mod live_bilibili_com;
mod space_bilibili_com;
pub(crate) mod twitter_com;

use std::{
    fmt::{self, Display},
    future::Future,
    pin::Pin,
};

use live_bilibili_com::LiveBilibiliComFetcher;
use space_bilibili_com::SpaceBilibiliComFetcher;
use twitter_com::TwitterComFetcher;

use crate::config::Platform;

#[derive(Debug)]
#[allow(clippy::enum_variant_names)]
pub enum PlatformName {
    LiveBilibiliCom,
    SpaceBilibiliCom,
    TwitterCom,
}

impl fmt::Display for PlatformName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LiveBilibiliCom => write!(f, "bilibili 直播"),
            Self::SpaceBilibiliCom => write!(f, "bilibili 动态"),
            Self::TwitterCom => write!(f, "Twitter"),
        }
    }
}

#[derive(Debug)]
pub struct StatusSource {
    pub platform_name: PlatformName,
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
    pub fn needs_notify<'a>(&'a self, last_status: Option<&'a Status>) -> Option<Notification<'a>> {
        match (&self.kind, last_status.map(|s| &s.kind)) {
            (StatusKind::Live(live_status), Some(StatusKind::Live(last_live_status))) => {
                (live_status.online && !last_live_status.online).then_some(Notification {
                    kind: NotificationKind::Live(live_status),
                    source: &self.source,
                })
            }
            (StatusKind::Posts(posts), Some(StatusKind::Posts(last_posts))) => {
                let new_posts =
                    vec_diff_by(&posts.0, &last_posts.0, |l, r| l.url == r.url).collect::<Vec<_>>();
                if !new_posts.is_empty() {
                    Some(Notification {
                        kind: NotificationKind::Posts(PostsRef(new_posts)),
                        source: &self.source,
                    })
                } else {
                    None
                }
            }
            (_, None) => None,
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
pub struct Post {
    pub content: String,
    pub url: String,
    pub is_repost: bool, // TODO: Include the source information
    pub is_quote: bool,  // TODO: Include the source information
    pub attachments: Vec<PostAttachment>,
}

#[derive(Debug)]
pub enum PostAttachment {
    Image(PostAttachmentImage),
    #[allow(dead_code)]
    Video(PostAttachmentVideo),
}

#[derive(Debug)]
pub struct PostAttachmentImage {
    pub media_url: String,
}

#[derive(Debug)]
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
        write!(f, "{:?}", self.0.iter().map(|p| &p.url))
    }
}

#[derive(Debug)]
pub struct PostsRef<'a>(pub Vec<&'a Post>);

impl fmt::Display for PostsRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0.iter().map(|p| &p.url))
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
    Live(&'a LiveStatus),
    Posts(PostsRef<'a>),
}

impl<'a> fmt::Display for NotificationKind<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Live(live_status) => write!(f, "{}", live_status),
            Self::Posts(posts) => write!(f, "{}", posts),
        }
    }
}

pub trait Fetcher: Display {
    fn fetch_status(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<Status>> + Send + '_>>;

    // "post" means "after" here
    fn post_filter_opt<'a>(
        &'a self,
        notification: Option<Notification<'a>>,
    ) -> Pin<Box<dyn Future<Output = Option<Notification<'a>>> + Send + '_>>
    where
        Self: Sync,
    {
        Box::pin(async move {
            match notification {
                Some(n) => self.post_filter(n).await,
                None => None,
            }
        })
    }

    // "post" means "after" here
    fn post_filter<'a>(
        &'a self,
        notification: Notification<'a>,
    ) -> Pin<Box<dyn Future<Output = Option<Notification<'a>>> + Send + '_>> {
        Box::pin(async move { Some(notification) })
    }
}

pub fn fetcher(platform: Platform) -> Box<dyn Fetcher + Send + Sync> {
    match platform {
        Platform::LiveBilibiliCom(p) => Box::new(LiveBilibiliComFetcher::new(p)),
        Platform::SpaceBilibiliCom(p) => Box::new(SpaceBilibiliComFetcher::new(p)),
        Platform::TwitterCom(p) => Box::new(TwitterComFetcher::new(p)),
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
    fn test_vec_diff() {
        assert_eq!(
            vec_diff(&[1, 2, 3], &[4, 2, 3, 4]).collect::<Vec<_>>(),
            [&1]
        )
    }
}
