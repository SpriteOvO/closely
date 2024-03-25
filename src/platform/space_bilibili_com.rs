use std::{collections::HashSet, fmt, future::Future, pin::Pin};

use anyhow::{anyhow, bail, Ok};
use serde::Deserialize;
use serde_json::{self as json};
use spdlog::prelude::*;
use tap::prelude::*;
use tokio::sync::{Mutex, OnceCell};

use super::{
    live_bilibili_com::BilibiliResponse, Fetcher, Notification, NotificationKind, PlatformName,
    Post, PostAttachment, PostAttachmentImage, Posts, Status, StatusKind, StatusSource, User,
};
use crate::{
    config::PlatformSpaceBilibiliCom,
    platform::{PostsRef, RepostFrom},
};

const SPACE_BILIBILI_COM_API: &str =
    "https://api.vc.bilibili.com/dynamic_svr/v2/dynamic_svr/space_history?host_uid=";

mod data {
    use super::*;

    #[derive(Debug, Deserialize)]
    pub struct SpaceHistory {
        pub has_more: u64,
        pub cards: Vec<Card>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Card {
        pub desc: Desc,
        pub card: String, // JSON serialized string
    }

    #[derive(Debug, Deserialize)]
    pub struct Desc {
        #[serde(rename = "type")]
        pub kind: u64,
        pub dynamic_id_str: String,
        pub origin: Option<Box<Desc>>,
    }

    //

    #[derive(Debug, Deserialize)]
    pub struct CardForwardPost {
        pub item: CardForwardPostItem,
        pub origin: String, // JSON serialized string
        pub user: CardForwardPostUser,
    }

    #[derive(Debug, Deserialize)]
    pub struct CardForwardPostItem {
        pub content: String,
        pub orig_type: u64,
    }

    #[derive(Debug, Deserialize)]
    pub struct CardForwardPostUser {
        pub face: String,
        pub uid: u64,
        pub uname: String,
    }

    impl From<CardForwardPostUser> for User {
        fn from(value: CardForwardPostUser) -> Self {
            Self {
                nickname: value.uname,
                profile_url: format!("https://space.bilibili.com/{}", value.uid),
                avatar_url: value.face,
            }
        }
    }

    //

    #[derive(Debug, Deserialize)]
    pub struct CardPublishArticle {
        pub author: CardPublishArticleAuthor,
        pub id: u64,
        pub summary: String,
        pub title: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct CardPublishArticleAuthor {
        pub face: String,
        pub mid: u64,
        pub name: String,
    }

    impl From<CardPublishArticleAuthor> for User {
        fn from(value: CardPublishArticleAuthor) -> Self {
            Self {
                nickname: value.name,
                profile_url: format!("https://space.bilibili.com/{}", value.mid),
                avatar_url: value.face,
            }
        }
    }

    //

    #[derive(Debug, Deserialize)]
    pub struct CardPostMedia {
        pub item: CardPostMediaItem,
        pub user: CardPostMediaUser,
    }

    #[derive(Debug, Deserialize)]
    pub struct CardPostMediaItem {
        pub description: String,
        pub pictures: Vec<CardPostMediaPicture>,
    }

    #[derive(Debug, Deserialize)]
    pub struct CardPostMediaPicture {
        pub img_src: String, // URL
    }

    #[derive(Debug, Deserialize)]
    pub struct CardPostMediaUser {
        pub head_url: String,
        pub name: String,
        pub uid: u64,
    }

    impl From<CardPostMediaUser> for User {
        fn from(value: CardPostMediaUser) -> Self {
            Self {
                nickname: value.name,
                profile_url: format!("https://space.bilibili.com/{}", value.uid),
                avatar_url: value.head_url,
            }
        }
    }

    //

    #[derive(Debug, Deserialize)]
    pub struct CardPostText {
        pub item: CardPostTextItem,
        pub user: CardPostTextUser,
    }

    #[derive(Debug, Deserialize)]
    pub struct CardPostTextItem {
        pub content: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct CardPostTextUser {
        pub face: String,
        pub uid: u64,
        pub uname: String,
    }

    impl From<CardPostTextUser> for User {
        fn from(value: CardPostTextUser) -> Self {
            Self {
                nickname: value.uname,
                profile_url: format!("https://space.bilibili.com/{}", value.uid),
                avatar_url: value.face,
            }
        }
    }

    //

    #[derive(Debug, Deserialize)]
    pub struct CardPublishVideo {
        pub desc: String, // Description of the video
        pub owner: CardPublishVideoOwner,
        pub pic: String, // Image URL
        pub title: String,
        pub short_link_v2: String, // URL
    }

    #[derive(Debug, Deserialize)]
    pub struct CardPublishVideoOwner {
        pub face: String,
        pub mid: u64,
        pub name: String,
    }

    impl From<CardPublishVideoOwner> for User {
        fn from(value: CardPublishVideoOwner) -> Self {
            Self {
                nickname: value.name,
                profile_url: format!("https://space.bilibili.com/{}", value.mid),
                avatar_url: value.face,
            }
        }
    }
}

pub struct SpaceBilibiliComFetcher {
    params: PlatformSpaceBilibiliCom,
    first_fetch: OnceCell<()>,
    fetched_cache: Mutex<HashSet<String>>,
}

impl Fetcher for SpaceBilibiliComFetcher {
    fn fetch_status(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<Status>> + Send + '_>> {
        Box::pin(self.fetch_status_impl())
    }

    fn post_filter<'a>(
        &'a self,
        notification: Notification<'a>,
    ) -> Pin<Box<dyn Future<Output = Option<Notification<'a>>> + Send + '_>> {
        Box::pin(self.post_filter_impl(notification))
    }
}

impl fmt::Display for SpaceBilibiliComFetcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.params)
    }
}

impl SpaceBilibiliComFetcher {
    pub fn new(params: PlatformSpaceBilibiliCom) -> Self {
        Self {
            params,
            first_fetch: OnceCell::new(),
            fetched_cache: Mutex::new(HashSet::new()),
        }
    }

    async fn fetch_status_impl(&self) -> anyhow::Result<Status> {
        let posts = fetch_space_bilibili_history(self.params.uid).await?;

        // The initial full cache for `post_filter`
        self.first_fetch
            .get_or_init(|| async {
                let mut published_cache = self.fetched_cache.lock().await;
                posts.0.iter().for_each(|post| {
                    assert!(published_cache.insert(post.url.clone()));
                })
            })
            .await;

        Ok(Status {
            kind: StatusKind::Posts(posts),
            source: StatusSource {
                platform_name: PlatformName::SpaceBilibiliCom,
                // TODO: User info is only contained in cards, not in a unique kv, implement it
                // later if needed
                user: None,
            },
        })
    }

    async fn post_filter_impl<'a>(
        &self,
        mut notification: Notification<'a>,
    ) -> Option<Notification<'a>> {
        // Sometimes the API returns posts without all "published video" posts, it
        // causes the problem that the next update will treat the missing posts as new
        // posts and notify them again. So we do some hacky filter here.

        if let NotificationKind::Posts(posts) = notification.kind {
            let mut fetched_cache = self.fetched_cache.lock().await;
            let remaining_posts = posts
                .0
                .into_iter()
                .filter(|post| !fetched_cache.contains(&post.url))
                .collect::<Vec<_>>();

            remaining_posts.iter().for_each(|post| {
                assert!(fetched_cache.insert(post.url.clone()));
            });
            drop(fetched_cache);

            if remaining_posts.is_empty() {
                return None;
            }
            notification.kind = NotificationKind::Posts(PostsRef(remaining_posts));
        }
        Some(notification)
    }
}

async fn fetch_space_bilibili_history(uid: u64) -> anyhow::Result<Posts> {
    let resp = reqwest::Client::new()
        .get(format!("{SPACE_BILIBILI_COM_API}{}", uid))
        .send()
        .await
        .map_err(|err| anyhow!("failed to send request: {err}"))?;

    let status = resp.status();
    if !status.is_success() {
        bail!("response status is not success: {resp:?}");
    }

    let text = resp
        .text()
        .await
        .map_err(|err| anyhow!("failed to obtain text from response: {err}"))?;

    let resp: BilibiliResponse<data::SpaceHistory> =
        json::from_str(&text).map_err(|err| anyhow!("failed to deserialize response: {err}"))?;
    if resp.code != 0 {
        bail!("response contains error, response '{text}'");
    }

    parse_response(resp.data)
}

fn parse_response(resp: data::SpaceHistory) -> anyhow::Result<Posts> {
    fn parse_card(desc: &data::Desc, card: &str) -> anyhow::Result<Post> {
        match desc.kind {
            // Deleted post
            0 => Ok(Post {
                user: User {
                    nickname: "未知用户".into(),
                    profile_url: "https://www.bilibili.com/".into(),
                    avatar_url: "https://i0.hdslb.com/bfs/face/member/noface.jpg".into(),
                },
                content: "源动态已被作者删除".into(),
                url: "https://www.bilibili.com/".into(),
                repost_from: None,
                attachments: vec![],
            }),
            // Forward post
            1 => {
                let forward_post = json::from_str::<data::CardForwardPost>(card)?;
                let Some(origin_desc) = &desc.origin else {
                    bail!(
                        "UNEXPECTED! forward post without origin. dynamic id: '{}'",
                        desc.dynamic_id_str
                    );
                };
                let repost_from = parse_card(origin_desc, &forward_post.origin)
                    .map_err(|err| anyhow!("failed to parse origin card: {err}"))?;
                Ok(Post {
                    user: forward_post.user.into(),
                    content: forward_post.item.content,
                    url: format!("https://t.bilibili.com/{}", desc.dynamic_id_str),
                    repost_from: Some(RepostFrom::Recursion(Box::new(repost_from))),
                    attachments: vec![],
                })
            }
            // Post media
            2 => {
                let post_media = json::from_str::<data::CardPostMedia>(card)?;
                Ok(Post {
                    user: post_media.user.into(),
                    content: post_media.item.description,
                    url: format!("https://t.bilibili.com/{}", desc.dynamic_id_str),
                    repost_from: None,
                    attachments: post_media
                        .item
                        .pictures
                        .into_iter()
                        .map(|picture| {
                            PostAttachment::Image(PostAttachmentImage {
                                media_url: picture.img_src,
                            })
                        })
                        .collect(),
                })
            }
            // Post text
            4 => {
                let post_media = json::from_str::<data::CardPostText>(card)?;
                Ok(Post {
                    user: post_media.user.into(),
                    content: post_media.item.content,
                    url: format!("https://t.bilibili.com/{}", desc.dynamic_id_str),
                    repost_from: None,
                    attachments: vec![],
                })
            }
            // Publish video
            8 => {
                let publish_video = json::from_str::<data::CardPublishVideo>(card)?;
                Ok(Post {
                    user: publish_video.owner.into(),
                    content: format!("投稿了视频《{}》", publish_video.title),
                    url: publish_video.short_link_v2,
                    repost_from: None,
                    attachments: vec![PostAttachment::Image(PostAttachmentImage {
                        media_url: publish_video.pic,
                    })],
                })
            }
            // Publish article
            64 => {
                let publish_article = json::from_str::<data::CardPublishArticle>(card)?;
                Ok(Post {
                    user: publish_article.author.into(),
                    // TODO: Add a link to the title
                    content: format!("投稿了文章《{}》", publish_article.title),
                    url: format!("https://www.bilibili.com/read/cv{}", publish_article.id),
                    repost_from: None,
                    attachments: vec![],
                })
            }
            _ => {
                bail!("unknown card type: {}", desc.kind);
            }
        }
    }

    let items = resp
        .cards
        .into_iter()
        .filter_map(|card| {
            parse_card(&card.desc, &card.card)
                .tap_err(|err| error!("failed to deserialize card: {err} for '{card:?}'"))
                .ok()
        })
        .collect();

    Ok(Posts(items))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deser() {
        let history = fetch_space_bilibili_history(8047632).await.unwrap();

        assert!(history.0.iter().all(|post| !post.url.is_empty()));
        assert!(history.0.iter().all(|post| !post.content.is_empty()));
    }

    #[tokio::test]
    async fn dedup_published_videos() {
        let fetcher = SpaceBilibiliComFetcher::new(PlatformSpaceBilibiliCom { uid: 1 });

        let source = StatusSource {
            platform_name: PlatformName::SpaceBilibiliCom,
            user: None,
        };
        let mut posts = vec![];

        macro_rules! make_notification {
            ( $posts:expr ) => {
                Notification {
                    kind: NotificationKind::Posts(PostsRef($posts)),
                    source: &source,
                }
            };
            () => {
                make_notification!(posts.iter().collect())
            };
        }

        assert!(fetcher.post_filter(make_notification!()).await.is_none());

        posts.push(Post {
            user: User {
                nickname: "test display name".into(),
                profile_url: "https://test.profile".into(),
                avatar_url: "https://test.avatar".into(),
            },
            content: "test1".into(),
            url: "https://test1".into(),
            repost_from: None,
            attachments: vec![],
        });

        let filtered = fetcher.post_filter(make_notification!()).await;
        assert!(matches!(
            filtered.unwrap().kind,
            NotificationKind::Posts(posts) if posts.0.len() == 1 && posts.0[0].content == "test1"
        ));

        let filtered = fetcher.post_filter(make_notification!()).await;
        assert!(filtered.is_none());

        let filtered = fetcher.post_filter(make_notification!(vec![])).await;
        assert!(filtered.is_none());

        posts.push(Post {
            user: User {
                nickname: "test display name".into(),
                profile_url: "https://test.profile".into(),
                avatar_url: "https://test.avatar".into(),
            },
            content: "test2".into(),
            url: "https://test2".into(),
            repost_from: None,
            attachments: vec![],
        });

        let filtered = fetcher.post_filter(make_notification!()).await;
        assert!(matches!(
            filtered.unwrap().kind,
            NotificationKind::Posts(posts) if posts.0.len() == 1 && posts.0[0].content == "test2"
        ));

        let filtered = fetcher.post_filter(make_notification!(vec![])).await;
        assert!(filtered.is_none());
    }
}
