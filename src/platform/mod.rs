mod live_bilibili_com;
mod twitter_com;

use std::fmt;

use anyhow::anyhow;
use spdlog::prelude::*;

use crate::config::Platform;

#[derive(Debug)]
pub enum PlatformName {
    LiveBilibiliCom,
    TwitterCom,
}

impl fmt::Display for PlatformName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LiveBilibiliCom => write!(f, "live.bilibili"),
            Self::TwitterCom => write!(f, "Twitter"),
        }
    }
}

#[derive(Debug)]
pub struct StatusFrom {
    pub platform_name: PlatformName,
    pub user_display_name: String,
    pub user_profile_url: String,
}

#[derive(Debug)]
pub struct Status {
    pub kind: StatusKind,
    pub from: StatusFrom,
}

impl Status {
    pub fn needs_notify<'a>(&'a self, last_status: Option<&'a Status>) -> Option<Notification<'a>> {
        match (&self.kind, last_status.map(|s| &s.kind)) {
            (StatusKind::Live(live_status), Some(StatusKind::Live(last_live_status))) => {
                (live_status.online && !last_live_status.online).then_some(Notification {
                    kind: NotificationKind::Live(live_status),
                    from: &self.from,
                })
            }
            (StatusKind::Posts(posts), Some(StatusKind::Posts(last_posts))) => {
                let new_posts =
                    vec_diff_by(&posts.0, &last_posts.0, |l, r| l.url == r.url).collect::<Vec<_>>();
                if !new_posts.is_empty() {
                    Some(Notification {
                        kind: NotificationKind::Posts(PostsRef(new_posts)),
                        from: &self.from,
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
    pub from: &'a StatusFrom,
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

pub async fn fetch_status(platform: &Platform) -> anyhow::Result<Status> {
    trace!("fetch status '{platform}'");

    match platform {
        Platform::LiveBilibiliCom(p) => live_bilibili_com::fetch_status(p).await,
        Platform::TwitterCom(p) => twitter_com::fetch_status(p).await,
    }
    .map_err(|err| anyhow!("({platform}) {err}"))
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
