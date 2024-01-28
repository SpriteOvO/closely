use std::{collections::HashSet, fmt, future::Future, pin::Pin};

use anyhow::{anyhow, bail, Ok};
use serde::Deserialize;
use serde_json::{self as json};
use spdlog::prelude::*;
use tap::prelude::*;
use tokio::sync::Mutex;

use super::{
    live_bilibili_com::BilibiliResponse, Fetcher, Notification, NotificationKind, PlatformName,
    Post, PostAttachment, PostAttachmentImage, Posts, Status, StatusKind, StatusSource,
};
use crate::{config::PlatformSpaceBilibiliCom, platform::PostsRef};

const SPACE_BILIBILI_COM_API: &str =
    "https://api.vc.bilibili.com/dynamic_svr/v1/dynamic_svr/space_history?host_uid=";

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
    }

    #[derive(Debug, Deserialize)]
    pub struct CardForwardPost {
        pub item: CardForwardPostItem,
        pub origin: String, // JSON serialized string
    }

    #[derive(Debug, Deserialize)]
    pub struct CardForwardPostItem {
        pub orig_type: u64,
    }

    #[derive(Debug, Deserialize)]
    pub struct CardPostText {
        pub item: CardPostTextItem,
    }

    #[derive(Debug, Deserialize)]
    pub struct CardPostTextItem {
        pub description: String,
        pub pictures: Vec<CardPostTextPicture>,
    }

    #[derive(Debug, Deserialize)]
    pub struct CardPostTextPicture {
        pub img_src: String, // URL
    }

    #[derive(Debug, Deserialize)]
    pub struct CardPublishVideo {
        pub desc: String, // Description of the video
        pub pic: String,  // Image URL
        pub title: String,
        pub short_link_v2: String, // URL
    }
}

pub struct SpaceBilibiliComFetcher {
    params: PlatformSpaceBilibiliCom,
    published_cache: Mutex<HashSet<String>>,
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
            published_cache: Mutex::new(HashSet::new()),
        }
    }

    async fn fetch_status_impl(&self) -> anyhow::Result<Status> {
        let posts = fetch_space_bilibili_history(self.params.uid).await?;

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
            let mut published_cache = self.published_cache.lock().await;
            let remaining_posts = posts
                .0
                .into_iter()
                .filter(|post| !published_cache.contains(&post.url))
                .collect::<Vec<_>>();

            remaining_posts.iter().for_each(|post| {
                assert!(published_cache.insert(post.url.clone()));
            });
            drop(published_cache);

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
    let items = resp
        .cards
        .into_iter()
        .filter_map(|card| -> Option<Post> {
            (|| -> anyhow::Result<Post> {
                match card.desc.kind {
                    // Forward post
                    1 => {
                        let _card = json::from_str::<data::CardForwardPost>(&card.card)?;
                        // TODO: Implement it after the common part of reposting is implemented
                        bail!("unimplemented")
                    }
                    // Post text
                    2 => {
                        let post_text = json::from_str::<data::CardPostText>(&card.card)?;
                        Ok(Post {
                            content: post_text.item.description,
                            url: format!("https://t.bilibili.com/{}", card.desc.dynamic_id_str),
                            is_repost: false,
                            is_quote: false,
                            attachments: post_text
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
                    // Publish video
                    8 => {
                        let publish_video = json::from_str::<data::CardPublishVideo>(&card.card)?;
                        Ok(Post {
                            content: format!("投稿了视频 {}", publish_video.title),
                            url: publish_video.short_link_v2,
                            is_repost: false,
                            is_quote: false,
                            attachments: vec![PostAttachment::Image(PostAttachmentImage {
                                media_url: publish_video.pic,
                            })],
                        })
                    }
                    _ => {
                        bail!("unknown card type: {}", card.desc.kind);
                    }
                }
            })()
            .tap_err(|err| {
                // TODO: See the above TODO
                if err.to_string() != "unimplemented" {
                    error!("failed to deserialize card: {err} for '{card:?}'")
                }
            })
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
        fetch_space_bilibili_history(8047632).await.unwrap();
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
            content: "test1".into(),
            url: "https://test1".into(),
            is_repost: false,
            is_quote: false,
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
            content: "test2".into(),
            url: "https://test2".into(),
            is_repost: false,
            is_quote: false,
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
