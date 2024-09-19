use std::{borrow::Cow, collections::HashSet, fmt, future::Future, ops::DerefMut, pin::Pin};

use anyhow::{anyhow, bail, Ok};
use once_cell::sync::Lazy;
use reqwest::header::{self, HeaderValue};
use serde::Deserialize;
use serde_json::{self as json};
use spdlog::prelude::*;
use tokio::sync::Mutex;

use super::*;
use crate::{
    platform::{PlatformMetadata, PlatformTrait},
    source::{
        FetcherTrait, Post, PostAttachment, PostAttachmentImage, PostContent, PostUrl, PostUrls,
        Posts, RepostFrom, Status, StatusKind, StatusSource, User,
    },
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigParams {
    pub user_id: u64,
}

impl fmt::Display for ConfigParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "space.bilibili.com:{}", self.user_id)
    }
}

mod data {
    use super::*;

    #[derive(Debug, Deserialize)]
    pub struct SpaceHistory {
        pub has_more: bool,
        pub items: Vec<Item>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Item {
        pub id_str: Option<String>, // `None` if the item is deleted
        pub modules: Modules,
        pub orig: Option<Box<Item>>,
    }

    //

    #[derive(Debug, Deserialize)]
    pub struct Modules {
        #[serde(rename = "module_author")]
        pub author: ModuleAuthor,
        #[serde(rename = "module_dynamic")]
        pub dynamic: ModuleDynamic,
    }

    #[derive(Clone, Debug, Deserialize)]
    #[serde(tag = "type")]
    pub enum ModuleAuthor {
        #[serde(rename = "AUTHOR_TYPE_NORMAL")]
        Normal(ModuleAuthorNormal),
        #[serde(rename = "AUTHOR_TYPE_PGC")]
        Pgc(ModuleAuthorPgc),
    }

    impl From<ModuleAuthor> for User {
        fn from(value: ModuleAuthor) -> Self {
            match value {
                ModuleAuthor::Normal(normal) => Self {
                    nickname: normal.name,
                    profile_url: format!("https://space.bilibili.com/{}", normal.mid),
                    avatar_url: normal.face,
                },
                ModuleAuthor::Pgc(pgc) => Self {
                    nickname: pgc.name,
                    profile_url: format!("https://bangumi.bilibili.com/anime/{}", pgc.mid),
                    avatar_url: pgc.face,
                },
            }
        }
    }

    #[derive(Clone, Debug, Deserialize)]
    pub struct ModuleAuthorNormal {
        pub face: String, // URL
        pub mid: u64,
        pub name: String,
        pub pub_ts: u64,
    }

    #[derive(Clone, Debug, Deserialize)]
    pub struct ModuleAuthorPgc {
        pub face: String, // URL
        pub mid: u64,
        pub name: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamic {
        pub desc: Option<RichText>,
        pub major: Option<ModuleDynamicMajor>,
    }

    //

    #[derive(Debug, Deserialize)]
    #[serde(tag = "type")]
    pub enum ModuleDynamicMajor {
        #[serde(rename = "MAJOR_TYPE_NONE")]
        None(ModuleDynamicMajorNone),
        #[serde(rename = "MAJOR_TYPE_OPUS")]
        Opus(ModuleDynamicMajorOpus),
        #[serde(rename = "MAJOR_TYPE_ARCHIVE")]
        Archive(ModuleDynamicMajorArchive),
        #[serde(rename = "MAJOR_TYPE_ARTICLE")]
        Article(ModuleDynamicMajorArticle),
        #[serde(rename = "MAJOR_TYPE_DRAW")]
        Draw(ModuleDynamicMajorDraw),
        #[serde(rename = "MAJOR_TYPE_COMMON")]
        Common(ModuleDynamicMajorCommon),
        #[serde(rename = "MAJOR_TYPE_PGC")]
        Pgc(ModuleDynamicMajorPgc),
        #[serde(rename = "MAJOR_TYPE_LIVE")]
        Live(ModuleDynamicMajorLive),
        #[serde(rename = "MAJOR_TYPE_LIVE_RCMD")]
        LiveRcmd, // We don't care about this item
        #[serde(rename = "MAJOR_TYPE_BLOCKED")]
        Blocked,
    }

    impl ModuleDynamicMajor {
        pub fn as_archive(&self) -> Option<&ModuleDynamicMajorArchiveInner> {
            match self {
                ModuleDynamicMajor::Archive(archive) => Some(&archive.archive),
                _ => None,
            }
        }
    }

    //

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicMajorNone {
        pub none: ModuleDynamicMajorNoneInner,
    }

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicMajorNoneInner {
        pub tips: String,
    }

    //

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicMajorOpus {
        pub opus: ModuleDynamicMajorOpusInner,
    }

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicMajorOpusInner {
        pub pics: Vec<ModuleDynamicMajorOpusPic>,
        pub summary: RichText,
        pub title: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicMajorOpusPic {
        pub url: String,
    }

    //

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicMajorArchive {
        pub archive: ModuleDynamicMajorArchiveInner,
    }

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicMajorArchiveInner {
        pub aid: String,   // AV ID
        pub bvid: String,  // BV ID
        pub cover: String, // URL
        pub desc: String,  // Description of the video
        pub duration_text: String,
        pub title: String, // Title of the video
        #[serde(rename = "type")]
        pub kind: u64, // Unknown
    }

    //

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicMajorArticle {
        pub article: ModuleDynamicMajorArticleInner,
    }

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicMajorArticleInner {
        pub covers: Vec<String>, // URLs
        pub desc: String,
        pub id: u64,
        pub title: String,
    }

    //

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicMajorDraw {
        pub draw: ModuleDynamicMajorDrawInner,
    }

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicMajorDrawInner {
        pub items: Vec<ModuleDynamicMajorDrawItem>,
    }

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicMajorDrawItem {
        pub src: String, // image URL
    }

    //

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicMajorCommon {
        pub common: ModuleDynamicMajorCommonInner,
    }

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicMajorCommonInner {
        pub cover: String, // URL
        pub desc: String,
        pub jump_url: String,
        pub title: String,
    }

    //

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicMajorPgc {
        pub pgc: ModuleDynamicMajorPgcInner,
    }

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicMajorPgcInner {
        pub cover: String, // URL
        pub epid: u64,
        pub title: String,
    }

    //

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicMajorLive {
        pub live: ModuleDynamicMajorLiveInner,
    }

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicMajorLiveInner {
        pub cover: String, // URL
        pub id: u64,
        pub title: String,
    }

    //

    #[derive(Debug, Deserialize)]
    pub struct RichText {
        // TODO: pub rich_text_nodes
        pub text: String,
    }
}

pub struct Fetcher {
    params: ConfigParams,
    // We cache all blocked posts and filter them again later, because the API sometimes
    // incorrectly returns fans-only posts for guests, this leads us to incorrectly assume that
    // these are new normal posts.
    blocked: Mutex<BlockedPostIds>,
}

impl PlatformTrait for Fetcher {
    fn metadata(&self) -> PlatformMetadata {
        PlatformMetadata {
            display_name: "bilibili 动态",
        }
    }
}

impl FetcherTrait for Fetcher {
    fn fetch_status(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<Status>> + Send + '_>> {
        Box::pin(self.fetch_status_impl())
    }
}

impl fmt::Display for Fetcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.params)
    }
}

impl Fetcher {
    pub fn new(params: ConfigParams) -> Self {
        Self {
            params,
            blocked: Mutex::new(BlockedPostIds(HashSet::new())),
        }
    }

    async fn fetch_status_impl(&self) -> anyhow::Result<Status> {
        let posts =
            fetch_space_history(self.params.user_id, self.blocked.lock().await.deref_mut()).await?;

        Ok(Status::new(
            StatusKind::Posts(posts),
            StatusSource {
                platform: self.metadata(),
                // TODO: User info is only contained in cards, not in a unique kv, implement it
                // later if needed
                user: None,
            },
        ))
    }
}

// Fans-only posts
struct BlockedPostIds(HashSet<String>);

#[allow(clippy::type_complexity)] // No, I don't think it's complex XD
static GUEST_COOKIES: Lazy<Mutex<Option<Vec<(String, String)>>>> = Lazy::new(|| Mutex::new(None));

async fn fetch_space_history(user_id: u64, blocked: &mut BlockedPostIds) -> anyhow::Result<Posts> {
    fetch_space_history_impl(user_id, blocked, true).await
}

fn fetch_space_history_impl<'a>(
    user_id: u64,
    blocked: &'a mut BlockedPostIds,
    retry: bool,
) -> Pin<Box<dyn Future<Output = anyhow::Result<Posts>> + Send + 'a>> {
    Box::pin(async move {
        let mut guest_cookies = GUEST_COOKIES.lock().await;
        if guest_cookies.is_none() {
            *guest_cookies = Some(
                obtain_guest_cookies()
                    .await
                    .map_err(|err| anyhow!("failed to obtain guest cookies: {err}"))?,
            );
        }
        let cookies = guest_cookies
            .as_ref()
            .unwrap()
            .iter()
            .map(|(name, value)| format!("{}={}", name, value))
            .collect::<Vec<_>>()
            .join("; ");
        let resp = bilibili_request_builder()?
            .get(format!(
                "https://api.bilibili.com/x/polymer/web-dynamic/v1/feed/space?host_mid={}&features=itemOpusStyle,listOnlyfans,opusBigCover,onlyfansVote,decorationCard,forwardListHidden,ugcDelete,onlyfansQaCard&dm_img_list=[]&dm_img_str=V2ViR0wgMS&dm_cover_img_str=REDACTED",
                user_id
            ))
            .header(header::COOKIE, HeaderValue::from_str(&cookies)?)
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

        let resp: Response<data::SpaceHistory> = json::from_str(&text)
            .map_err(|err| anyhow!("failed to deserialize response: {err}"))?;

        match resp.code {
            0 => {} // Success
            -352 => {
                // Auth error
                if retry {
                    // Invalidate the guest cookies and retry
                    *guest_cookies = None;
                    drop(guest_cookies);
                    warn!("bilibili guest token expired, retrying with new token");
                    return fetch_space_history_impl(user_id, blocked, false).await;
                } else {
                    bail!("bilibili failed with token expired, and already retried once")
                }
            }
            _ => bail!("response contains error, response '{text}'"),
        }

        parse_response(resp.data.unwrap(), blocked)
    })
}

fn parse_response(resp: data::SpaceHistory, blocked: &mut BlockedPostIds) -> anyhow::Result<Posts> {
    fn parse_item(item: &data::Item, parent_item: Option<&data::Item>) -> anyhow::Result<Post> {
        debug_assert!(matches!(
            item.modules
                .dynamic
                .major
                .as_ref()
                .and_then(|major| major.as_archive())
                .map(|archive| archive.kind),
            Some(1) | None
        ));

        let major_content =
            item.modules
                .dynamic
                .major
                .as_ref()
                .and_then(|major| -> Option<Cow<str>> {
                    match major {
                        data::ModuleDynamicMajor::None(none) => {
                            Some(Cow::Borrowed(&none.none.tips))
                        }
                        data::ModuleDynamicMajor::Opus(opus) => {
                            if let Some(title) = opus.opus.title.as_deref() {
                                Some(Cow::Owned(format!("{title}\n\n{}", opus.opus.summary.text)))
                            } else {
                                Some(Cow::Borrowed(&opus.opus.summary.text))
                            }
                        }
                        data::ModuleDynamicMajor::Archive(archive) => Some(Cow::Owned(format!(
                            "投稿了视频《{}》",
                            archive.archive.title
                        ))),
                        data::ModuleDynamicMajor::Article(article) => Some(Cow::Owned(format!(
                            "投稿了文章《{}》",
                            article.article.title
                        ))),
                        data::ModuleDynamicMajor::Draw(_) => None,
                        data::ModuleDynamicMajor::Common(common) => Some(Cow::Owned(format!(
                            "{} - {}",
                            common.common.title, common.common.desc
                        ))),
                        data::ModuleDynamicMajor::Pgc(pgc) => {
                            Some(Cow::Owned(format!("番剧《{}》", pgc.pgc.title)))
                        }
                        data::ModuleDynamicMajor::Live(live) => {
                            Some(Cow::Borrowed(&live.live.title))
                        }
                        data::ModuleDynamicMajor::LiveRcmd | data::ModuleDynamicMajor::Blocked => {
                            critical!("unexpected major type: {major:?}");
                            unreachable!()
                        }
                    }
                });
        let content = match (&item.modules.dynamic.desc, major_content) {
            (Some(desc), Some(major)) => format!("{}\n\n{}", desc.text, major),
            (Some(desc), None) => desc.text.clone(),
            (None, Some(major)) => major.into(),
            (None, None) => bail!("item no content. item: {item:?}"),
        };

        let original = item
            .orig
            .as_ref()
            .map(|orig| parse_item(orig, Some(item)))
            .transpose()
            .map_err(|err| anyhow!("failed to parse origin card: {err}"))?;

        let major_url = if let Some(id_str) = item.id_str.as_ref() {
            PostUrl::new_clickable(
                format!("https://www.bilibili.com/opus/{id_str}"),
                "查看动态",
            )
        } else {
            PostUrl::Identity(format!(
                "https://www.bilibili.com/opus/{}::forward-original",
                parent_item.unwrap().id_str.as_ref().unwrap()
            ))
        };
        let mut urls = vec![major_url];
        item.modules
            .dynamic
            .major
            .as_ref()
            .inspect(|major| match major {
                data::ModuleDynamicMajor::None(_)
                | data::ModuleDynamicMajor::Opus(_)
                | data::ModuleDynamicMajor::Draw(_)
                | data::ModuleDynamicMajor::Common(_) => {
                    // No need to add extra URLs
                }
                data::ModuleDynamicMajor::Archive(archive) => urls.push(PostUrl::new_clickable(
                    format!("https://www.bilibili.com/video/{}", archive.archive.bvid),
                    "查看视频",
                )),
                data::ModuleDynamicMajor::Article(article) => urls.push(PostUrl::new_clickable(
                    format!("https://www.bilibili.com/read/cv{}", article.article.id),
                    "查看文章",
                )),
                data::ModuleDynamicMajor::Pgc(pgc) => urls.push(PostUrl::new_clickable(
                    format!("https://www.bilibili.com/bangumi/play/ep{}", pgc.pgc.epid),
                    "查看文章",
                )),
                data::ModuleDynamicMajor::Live(live) => urls.push(PostUrl::new_clickable(
                    format!("https://live.bilibili.com/{}", live.live.id),
                    "前往直播间",
                )),
                data::ModuleDynamicMajor::LiveRcmd | data::ModuleDynamicMajor::Blocked => {
                    critical!("unexpected major type: {major:?}");
                    unreachable!()
                }
            });

        Ok(Post {
            user: Some(item.modules.author.clone().into()),
            content: PostContent::plain(content),
            urls: PostUrls::from_iter(urls)?,
            repost_from: original.map(|original| RepostFrom::Recursion(Box::new(original))),
            attachments: item
                .modules
                .dynamic
                .major
                .as_ref()
                .map(|major| match major {
                    data::ModuleDynamicMajor::None(_) => vec![],
                    data::ModuleDynamicMajor::Opus(opus) => opus
                        .opus
                        .pics
                        .iter()
                        .map(|pic| {
                            PostAttachment::Image(PostAttachmentImage {
                                media_url: upgrade_to_https(&pic.url),
                                has_spoiler: false,
                            })
                        })
                        .collect(),
                    data::ModuleDynamicMajor::Archive(archive) => {
                        vec![PostAttachment::Image(PostAttachmentImage {
                            media_url: upgrade_to_https(&archive.archive.cover),
                            has_spoiler: false,
                        })]
                    }
                    data::ModuleDynamicMajor::Article(article) => article
                        .article
                        .covers
                        .iter()
                        .map(|cover| {
                            PostAttachment::Image(PostAttachmentImage {
                                media_url: upgrade_to_https(cover),
                                has_spoiler: false,
                            })
                        })
                        .collect(),
                    data::ModuleDynamicMajor::Draw(draw) => draw
                        .draw
                        .items
                        .iter()
                        .map(|item| {
                            PostAttachment::Image(PostAttachmentImage {
                                media_url: upgrade_to_https(&item.src),
                                has_spoiler: false,
                            })
                        })
                        .collect(),
                    data::ModuleDynamicMajor::Common(common) => {
                        vec![PostAttachment::Image(PostAttachmentImage {
                            media_url: upgrade_to_https(&common.common.cover),
                            has_spoiler: false,
                        })]
                    }
                    data::ModuleDynamicMajor::Pgc(pgc) => {
                        vec![PostAttachment::Image(PostAttachmentImage {
                            media_url: upgrade_to_https(&pgc.pgc.cover),
                            has_spoiler: false,
                        })]
                    }
                    data::ModuleDynamicMajor::Live(live) => {
                        vec![PostAttachment::Image(PostAttachmentImage {
                            media_url: upgrade_to_https(&live.live.cover),
                            has_spoiler: false,
                        })]
                    }
                    data::ModuleDynamicMajor::LiveRcmd | data::ModuleDynamicMajor::Blocked => {
                        critical!("unexpected major type: {major:?}");
                        unreachable!()
                    }
                })
                .unwrap_or_default(),
        })
    }

    let items = resp
        .items
        .into_iter()
        .filter(|item| {
            !matches!(
                item.modules.dynamic.major,
                Some(data::ModuleDynamicMajor::LiveRcmd)
            )
        })
        .filter(|item| {
            item.id_str
                .as_deref()
                .map(|id_str| {
                    if matches!(
                        item.modules.dynamic.major,
                        Some(data::ModuleDynamicMajor::Blocked)
                    ) {
                        blocked.0.insert(id_str.into());
                        false
                    } else if blocked.0.contains(id_str) {
                        let rustfmt_bug = "filtered out a bilibili space item";
                        let rustfmt_bug2 = "as it was blocked and probobly a fans-only post";
                        warn!("{rustfmt_bug} '{id_str}' {rustfmt_bug2}");
                        false
                    } else {
                        true
                    }
                })
                .unwrap_or(true)
        })
        .filter_map(|item| {
            parse_item(&item, None)
                .inspect_err(|err| error!("failed to deserialize item: {err} for '{item:?}'"))
                .ok()
        })
        .collect();

    Ok(Posts(items))
}

async fn obtain_guest_cookies() -> anyhow::Result<Vec<(String, String)>> {
    // Okay, I gave up on cracking the auth process
    use headless_chrome::{Browser, LaunchOptionsBuilder};

    let browser = Browser::new(
        LaunchOptionsBuilder::default()
            // https://github.com/rust-headless-chrome/rust-headless-chrome/issues/267
            .sandbox(false)
            .build()?,
    )?;
    let tab = browser.new_tab()?;
    tab.navigate_to("https://space.bilibili.com/8047632/dynamic")?;
    tab.wait_until_navigated()?;

    let kvs = tab
        .get_cookies()?
        .into_iter()
        .map(|cookie| (cookie.name, cookie.value))
        .collect();
    Ok(kvs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deser() {
        let mut blocked = BlockedPostIds(HashSet::new());

        let history = fetch_space_history(8047632, &mut blocked).await.unwrap();
        assert!(history.0.iter().all(|post| !post
            .urls
            .major()
            .as_clickable()
            .unwrap()
            .url
            .is_empty()));
        assert!(history.0.iter().all(|post| !post.content.is_empty()));

        let history = fetch_space_history(178362496, &mut blocked).await.unwrap();
        assert!(history.0.iter().all(|post| !post
            .urls
            .major()
            .as_clickable()
            .unwrap()
            .url
            .is_empty()));
        assert!(history.0.iter().all(|post| !post.content.is_empty()));
    }
}
