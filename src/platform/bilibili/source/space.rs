use std::{
    borrow::Cow,
    collections::HashSet,
    fmt::{self, Display},
    future::Future,
    ops::DerefMut,
    pin::Pin,
    str::FromStr,
    sync::{Arc, Mutex as StdMutex},
};

use anyhow::{anyhow, bail, ensure};
use chrono::DateTime;
use reqwest::{Url, header::COOKIE};
use serde::Deserialize;
use serde_json::{self as json};
use spdlog::prelude::*;
use tokio::sync::Mutex;

use super::super::{Response, upgrade_to_https};
use crate::{
    config::{Accessor, AsSecretRef, Config, Validator},
    platform::{PlatformMetadata, PlatformTraitStatic, bilibili::bilibili_request_builder},
    prop,
    source::{
        FetcherTrait, Post, PostAttachment, PostAttachmentImage, PostContent, PostUrl, PostUrls,
        Posts, RepostFrom, Status, StatusKind, StatusSource, User,
    },
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigParams {
    pub user_id: u64,
}

impl Validator for ConfigParams {
    fn validate(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl fmt::Display for ConfigParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "space.bilibili.com:{}", self.user_id)
    }
}

enum TrimRule<T> {
    Remove,
    Replace(T),
    Keep,
}

mod data {
    use super::*;
    use crate::source::PostContentPart;

    #[derive(Clone, Debug, Deserialize)]
    #[serde(untagged, deny_unknown_fields)]
    pub enum StrOrNumber {
        Str(String),
        Number(u64),
    }

    impl StrOrNumber {
        pub fn try_to_number(&self) -> Option<u64> {
            match self {
                Self::Str(s) => s.parse::<u64>().ok(),
                Self::Number(n) => Some(*n),
            }
        }
    }

    impl Display for StrOrNumber {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                StrOrNumber::Str(s) => write!(f, "{s}"),
                StrOrNumber::Number(n) => write!(f, "{n}"),
            }
        }
    }

    #[derive(Debug, Deserialize)]
    pub struct SpaceHistory {
        pub has_more: bool,
        pub items: Vec<Item>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Item {
        // `None` if the item is deleted
        pub id_str: Option<StrOrNumber /* WTF? bilibili devs? */>,
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
        #[serde(rename = "module_tag")]
        pub tag: Option<ModuleTag>,
    }

    #[derive(Debug, Deserialize)]
    pub struct ModuleTag {
        pub text: String,
    }

    #[derive(Clone, Debug, Deserialize)]
    #[serde(tag = "type")]
    pub enum ModuleAuthor {
        #[serde(rename = "AUTHOR_TYPE_NORMAL")]
        Normal(ModuleAuthorNormal),
        #[serde(rename = "AUTHOR_TYPE_PGC")]
        Pgc(ModuleAuthorPgc),
    }

    impl ModuleAuthor {
        pub fn pub_time(&self) -> Option<u64> {
            let pub_ts = match self {
                ModuleAuthor::Normal(normal) => &normal.pub_ts,
                ModuleAuthor::Pgc(pgc) => &pgc.pub_ts,
            }
            .try_to_number()?;
            (pub_ts != 0).then_some(pub_ts)
        }
    }

    impl From<ModuleAuthor> for User {
        fn from(value: ModuleAuthor) -> Self {
            match value {
                ModuleAuthor::Normal(normal) => Self {
                    nickname: normal.name,
                    profile_url: format!("https://space.bilibili.com/{}", normal.mid),
                    avatar_url: Some(normal.face),
                },
                ModuleAuthor::Pgc(pgc) => Self {
                    nickname: pgc.name,
                    profile_url: format!("https://bangumi.bilibili.com/anime/{}", pgc.mid),
                    avatar_url: Some(pgc.face),
                },
            }
        }
    }

    #[derive(Clone, Debug, Deserialize)]
    pub struct ModuleAuthorNormal {
        pub face: String, // URL
        pub mid: StrOrNumber,
        pub name: String,
        pub pub_ts: StrOrNumber,
    }

    #[derive(Clone, Debug, Deserialize)]
    pub struct ModuleAuthorPgc {
        pub face: String, // URL
        pub mid: StrOrNumber,
        pub name: String,
        pub pub_ts: StrOrNumber, // Always 0?
    }

    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamic {
        pub desc: Option<RichText>,
        pub major: Option<ModuleDynamicMajor>,
        pub additional: Option<ModuleDynamicAdditional>,
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
        pub fn to_content(&self) -> Option<PostContent> {
            match self {
                Self::None(none) => Some(PostContent::plain(&none.none.tips)),
                Self::Opus(opus) => {
                    if let Some(title) = opus.opus.title.as_deref()
                        && !title.is_empty()
                    {
                        Some(
                            PostContent::plain(title)
                                .with_plain("\n\n")
                                .with_content(opus.opus.summary.to_content()),
                        )
                    } else {
                        Some(opus.opus.summary.to_content())
                    }
                }
                Self::Archive(archive) => Some(
                    PostContent::plain("投稿了视频《")
                        .with_plain(&archive.archive.title)
                        .with_plain("》"),
                ),
                Self::Article(article) => Some(
                    PostContent::plain("投稿了文章《")
                        .with_plain(&article.article.title)
                        .with_plain("》"),
                ),
                Self::Draw(_) => None,
                Self::Common(common) => Some(
                    PostContent::plain(&common.common.title)
                        .with_plain(" - ")
                        .with_plain(&common.common.desc),
                ),
                Self::Pgc(pgc) => Some(
                    PostContent::plain("剧集《")
                        .with_plain(&pgc.pgc.title)
                        .with_plain("》"),
                ),
                Self::Live(live) => Some(PostContent::plain(&live.live.title)),
                Self::LiveRcmd | Self::Blocked => {
                    critical!("unexpected major type", kv: { major:? = self });
                    unreachable!()
                }
            }
        }

        pub fn to_url(&self) -> Option<PostUrl> {
            match self {
                Self::None(_) | Self::Opus(_) | Self::Draw(_) | Self::Common(_) => {
                    // No need to add extra URLs
                    None
                }
                Self::Archive(archive) => Some(PostUrl::new_clickable(
                    format!("https://www.bilibili.com/video/{}", archive.archive.bvid),
                    "查看视频",
                )),
                Self::Article(article) => Some(PostUrl::new_clickable(
                    format!("https://www.bilibili.com/read/cv{}", article.article.id),
                    "查看文章",
                )),
                Self::Pgc(pgc) => Some(PostUrl::new_clickable(
                    format!("https://www.bilibili.com/bangumi/play/ep{}", pgc.pgc.epid),
                    "查看剧集",
                )),
                Self::Live(live) => Some(PostUrl::new_clickable(
                    format!("https://live.bilibili.com/{}", live.live.id),
                    "前往直播间",
                )),
                Self::LiveRcmd | Self::Blocked => {
                    critical!("unexpected major type", kv: { major:? = self });
                    unreachable!()
                }
            }
        }

        pub fn to_attachments(&self) -> Vec<PostAttachment> {
            match self {
                Self::None(_) => vec![],
                Self::Opus(opus) => opus
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
                Self::Archive(archive) => {
                    vec![PostAttachment::Image(PostAttachmentImage {
                        media_url: upgrade_to_https(&archive.archive.cover),
                        has_spoiler: false,
                    })]
                }
                Self::Article(article) => article
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
                Self::Draw(draw) => draw
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
                Self::Common(common) => {
                    vec![PostAttachment::Image(PostAttachmentImage {
                        media_url: upgrade_to_https(&common.common.cover),
                        has_spoiler: false,
                    })]
                }
                Self::Pgc(pgc) => {
                    vec![PostAttachment::Image(PostAttachmentImage {
                        media_url: upgrade_to_https(&pgc.pgc.cover),
                        has_spoiler: false,
                    })]
                }
                Self::Live(live) => {
                    vec![PostAttachment::Image(PostAttachmentImage {
                        media_url: upgrade_to_https(&live.live.cover),
                        has_spoiler: false,
                    })]
                }
                Self::LiveRcmd | Self::Blocked => {
                    critical!("unexpected major type", kv: { major:? = self });
                    unreachable!()
                }
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
        rich_text_nodes: Vec<RichTextNode>,
        // text: String, // Fallback
    }

    impl RichText {
        pub fn to_content(&self) -> PostContent {
            if self.rich_text_nodes.is_empty() {
                return PostContent::new();
            }

            // Sometimes the last node is just a newline or ends with a newline, we trim it
            // here
            let last_node_trim_end_rule = self.rich_text_nodes.last().unwrap().trim_end_rule();
            let rich_text_nodes: Vec<_> = match &last_node_trim_end_rule {
                TrimRule::Keep => self.rich_text_nodes[..].iter().collect(),
                TrimRule::Remove => self.rich_text_nodes[..self.rich_text_nodes.len() - 1]
                    .iter()
                    .collect(),
                TrimRule::Replace(node) => self.rich_text_nodes[..self.rich_text_nodes.len() - 1]
                    .iter()
                    .chain([node])
                    .collect(),
            };

            PostContent::from_parts(rich_text_nodes.iter().map(|node| match &node.kind {
                RichTextNodeKind::Text => PostContentPart::Plain(node.text.clone()),
                RichTextNodeKind::Web { jump_url } => PostContentPart::Link {
                    display: node.text.clone(),
                    url: jump_url.clone(),
                },
                RichTextNodeKind::At { rid } => PostContentPart::Link {
                    display: node.text.clone(),
                    url: format!("https://space.bilibili.com/{rid}"),
                },
                RichTextNodeKind::Topic { jump_url } => {
                    if jump_url.starts_with("//search.bilibili.com") {
                        PostContentPart::Link {
                            display: node.text.clone(),
                            url: format!("https:{jump_url}"),
                        }
                    } else {
                        warn!("unexpected bilibili topic URL in rich text node", kv: { jump_url: });
                        PostContentPart::Plain(node.orig_text.clone())
                    }
                }
                RichTextNodeKind::Bv { rid, .. } => PostContentPart::Link {
                    display: node.text.clone(),
                    url: format!("https://www.bilibili.com/video/{rid}"),
                },
                RichTextNodeKind::ViewPicture { pics, .. } => {
                    if pics.len() != 1 {
                        warn!("bilibili rich text view-pic node has unexpected number of pics", kv: { len = pics.len() });
                        PostContentPart::Plain(node.orig_text.clone())
                    } else {
                        PostContentPart::InlineAttachment(PostAttachment::Image(
                            PostAttachmentImage {
                                media_url: pics.first().unwrap().src.clone(),
                                has_spoiler: false,
                            },
                        ))
                    }
                }
                RichTextNodeKind::Goods { jump_url } => PostContentPart::Link {
                    display: node.text.clone(),
                    url: jump_url.clone(),
                },
                RichTextNodeKind::OgvEp { jump_url, rid } => PostContentPart::Link {
                    display: rid.clone(),
                    url: jump_url.clone(),
                },
                // We treat these nodes as plain text
                RichTextNodeKind::Emoji { .. }
                | RichTextNodeKind::Lottery { .. }
                | RichTextNodeKind::Vote => PostContentPart::Plain(node.orig_text.clone()),
                RichTextNodeKind::Unknown(info) => {
                    warn!("unexpected bilibili rich text node", kv: { info: });
                    PostContentPart::Plain(node.orig_text.clone())
                }
            }))
        }
    }

    #[derive(Debug, Deserialize)]
    pub struct RichTextNode {
        pub orig_text: String,
        pub text: String,
        #[serde(flatten)]
        pub kind: RichTextNodeKind,
    }

    impl RichTextNode {
        fn is_newline(&self) -> bool {
            matches!(&self.kind, RichTextNodeKind::Text)
                && self.text == "\n"
                && self.orig_text == "\n"
        }

        fn ends_with_newline(&self) -> bool {
            matches!(&self.kind, RichTextNodeKind::Text)
                && self.text.ends_with("\n")
                && self.orig_text.ends_with("\n")
        }

        // "\n" -> Remove
        // "text\n" -> Replace("text")
        // "text" -> Keep
        fn trim_end_rule(&self) -> TrimRule<Self> {
            if self.is_newline() {
                TrimRule::Remove
            } else if self.ends_with_newline() {
                TrimRule::Replace(Self {
                    orig_text: self.orig_text.trim_end_matches("\n").into(),
                    text: self.text.trim_end_matches("\n").into(),
                    kind: RichTextNodeKind::Text,
                })
            } else {
                TrimRule::Keep
            }
        }
    }

    #[derive(Debug, Deserialize)]
    #[serde(tag = "type")]
    pub enum RichTextNodeKind {
        #[serde(rename = "RICH_TEXT_NODE_TYPE_TEXT")]
        Text,
        #[serde(rename = "RICH_TEXT_NODE_TYPE_EMOJI")]
        Emoji { emoji: RichTextNodeEmoji },
        #[serde(rename = "RICH_TEXT_NODE_TYPE_WEB")]
        Web { jump_url: String },
        #[serde(rename = "RICH_TEXT_NODE_TYPE_AT")]
        At { rid: String },
        #[serde(rename = "RICH_TEXT_NODE_TYPE_TOPIC")]
        Topic { jump_url: String },
        #[serde(rename = "RICH_TEXT_NODE_TYPE_LOTTERY")]
        Lottery { rid: String },
        #[serde(rename = "RICH_TEXT_NODE_TYPE_BV")]
        Bv { jump_url: String, rid: String },
        #[serde(rename = "RICH_TEXT_NODE_TYPE_VIEW_PICTURE")]
        ViewPicture {
            jump_url: String,
            pics: Vec<RichTextNodeViewPicturePic>,
            rid: String,
        },
        #[serde(rename = "RICH_TEXT_NODE_TYPE_GOODS")]
        Goods { jump_url: String },
        #[serde(rename = "RICH_TEXT_NODE_TYPE_VOTE")]
        Vote,
        #[serde(rename = "RICH_TEXT_NODE_TYPE_OGV_EP")]
        OgvEp {
            jump_url: String,
            // FIXME: This is strange. Our return results do not contain `text` field, but it is
            // contained in the real browser. I have checked that the request parameters and
            // User-Agent are consistent with the real browser. As a workaround, we use the `rid`
            // field for display.
            //
            // text: String
            rid: String,
        },
        #[serde(untagged)]
        Unknown(json::Value),
    }

    #[derive(Debug, Deserialize)]
    pub struct RichTextNodeEmoji {
        pub icon_url: String,
        pub text: String,
        // pub size: u64
        // pub type: u64
    }

    #[derive(Debug, Deserialize)]
    pub struct RichTextNodeViewPicturePic {
        pub src: String,
        // pub height: u64
        // pub width: u64
        // pub size: u64
    }

    //

    #[derive(Debug, Deserialize)]
    #[serde(tag = "type")]
    pub enum ModuleDynamicAdditional {
        #[serde(rename = "ADDITIONAL_TYPE_RESERVE")]
        Reserve { reserve: ModuleDynamicReserve },
        #[serde(untagged)]
        Unknown(json::Value),
    }

    impl ModuleDynamicAdditional {
        pub fn to_content(&self) -> Option<PostContent> {
            match self {
                Self::Reserve { reserve } => {
                    let mut content = PostContent::plain(&reserve.title);
                    if let Some(desc1) = &reserve.desc1 {
                        content = content.with_plain("\n").with_plain(&desc1.text);
                    }
                    // Ignore `desc2`
                    if let Some(desc3) = &reserve.desc3 {
                        warn!("bilibili reservation desc3 is not null", kv: { desc3:? });
                    }
                    Some(content)
                }
                Self::Unknown(data) => {
                    warn!("unknown bilibili additional variant", kv: { data: });
                    None
                }
            }
        }
    }

    #[rustfmt::skip] // TODO: https://github.com/rust-lang/rustfmt/issues/6755
    #[derive(Debug, Deserialize)]
    pub struct ModuleDynamicReserve {
        pub title: String,
        pub desc1: Option<ReserveDescription>, // "YY-DD HH:MM 直播" (UTC+8?)
        pub desc2: Option<ReserveDescription>, // "XX人预约"
        pub desc3: Option<ReserveDescription>, // null
        // pub reserve_total: u64,
    }

    #[derive(Debug, Deserialize)]
    pub struct ReserveDescription {
        pub text: String,
    }
}

pub struct Fetcher {
    params: Accessor<ConfigParams>,
    // We cache all blocked posts and filter them again later, because the API sometimes
    // incorrectly returns fans-only posts for guests, this leads us to incorrectly assume that
    // these are new normal posts.
    blocked: Mutex<BlockedPostIds>,
}

impl PlatformTraitStatic for Fetcher {
    fn metadata() -> PlatformMetadata {
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
    pub fn new(params: Accessor<ConfigParams>) -> Self {
        Self {
            params,
            blocked: Mutex::new(BlockedPostIds(HashSet::new())),
        }
    }

    fn cookies(&self) -> Option<Cow<'_, str>> {
        Config::global().platform().bilibili.as_ref().and_then(|b| {
            b.cookies
                .as_ref()
                .map(|c| c.as_secret_ref().get_str().unwrap())
        })
    }

    async fn fetch_status_impl(&self) -> anyhow::Result<Status> {
        let posts = fetch_space_history(
            self.params.user_id,
            self.blocked.lock().await.deref_mut(),
            self.cookies(),
        )
        .await?;

        Ok(Status::new(
            StatusKind::Posts(posts),
            StatusSource {
                platform: Self::metadata(),
                // TODO: User info is only contained in cards, not in a unique kv, implement it
                // later if needed
                user: None,
            },
        ))
    }
}

// Fans-only posts
struct BlockedPostIds(HashSet<String>);

async fn fetch_space_history(
    user_id: u64,
    blocked: &mut BlockedPostIds,
    cookies: Option<Cow<'_, str>>,
) -> anyhow::Result<Posts> {
    let res = fetch_space_history_impl(user_id, blocked, cookies).await;
    if let Err(FetchSpaceHistoryError::Auth(true)) = res {
        warn!("bilibili space auth error with cookies used, retrying headless without cookies");
        Ok(fetch_space_history_impl(user_id, blocked, None).await?)
    } else {
        Ok(res?)
    }
}

#[derive(thiserror::Error, Debug)]
enum FetchSpaceHistoryError {
    #[error("auth error (cookies used: {0})")]
    Auth(bool),
    #[error("{0}")]
    Anyhow(#[from] anyhow::Error),
    #[error("response contains error, response '{0}'")]
    Others(String),
}

fn fetch_space_history_impl<'a>(
    user_id: u64,
    blocked: &'a mut BlockedPostIds,
    cookies: Option<Cow<'a, str>>,
) -> Pin<Box<dyn Future<Output = Result<Posts, FetchSpaceHistoryError>> + Send + 'a>> {
    Box::pin(async move {
        let cookies_used = cookies.is_some();
        let (status, text) = fetch_space(user_id, cookies)
            .await
            .map_err(|err| anyhow!("failed to send request: {err}"))?;
        if status != 200 {
            return Err(anyhow!("response status is not success: {text:?}").into());
        }

        let resp: Response<data::SpaceHistory> = json::from_str(&text)
            .map_err(|err| anyhow!("failed to deserialize response: {err}"))?;

        match resp.code {
            0 => {} // Success
            -352 => return Err(FetchSpaceHistoryError::Auth(cookies_used)),
            _ => return Err(FetchSpaceHistoryError::Others(text)),
        }

        Ok(parse_response(resp.data.unwrap(), blocked)?)
    })
}

fn parse_response(resp: data::SpaceHistory, blocked: &mut BlockedPostIds) -> anyhow::Result<Posts> {
    fn parse_item(item: &data::Item, parent_item: Option<&data::Item>) -> anyhow::Result<Post> {
        let desc_content = item
            .modules
            .dynamic
            .desc
            .as_ref()
            .map(|desc| desc.to_content());
        let major_content = item
            .modules
            .dynamic
            .major
            .as_ref()
            .and_then(|major| major.to_content());
        let content = match (desc_content, major_content) {
            (Some(desc), Some(major)) => desc.with_plain("\n\n").with_content(major),
            (Some(desc), None) => desc,
            (None, Some(major)) => major,
            (None, None) => bail!("item no content. item: {item:?}"),
        };

        let event = item
            .modules
            .dynamic
            .additional
            .as_ref()
            .and_then(|additional| additional.to_content());

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
        let url = item
            .modules
            .dynamic
            .major
            .as_ref()
            .and_then(|major| major.to_url())
            .unwrap_or(major_url);

        let time = item
            .modules
            .author
            .pub_time()
            .or(parent_item.and_then(|p| p.modules.author.pub_time()));
        ensure!(
            time.is_some(),
            "bilibili space found a post with no time: '{item:?}'"
        );
        let time = DateTime::from_timestamp(time.unwrap() as i64, 0)
            .ok_or_else(|| anyhow!("invalid pub time, url={url:?}"))?
            .into();

        let is_pinned = item
            .modules
            .tag
            .as_ref()
            .is_some_and(|tag| tag.text == "置顶");

        Ok(Post {
            user: item.modules.author.clone().into(),
            content,
            event,
            urls: PostUrls::new(url),
            time,
            is_pinned,
            repost_from: original.map(RepostFrom::new_quote),
            attachments: item
                .modules
                .dynamic
                .major
                .as_ref()
                .map(|major| major.to_attachments())
                .unwrap_or_default(),
            prefer_treat_as_reply: false,
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
                .as_ref()
                .map(|id_str| {
                    if matches!(
                        item.modules.dynamic.major,
                        Some(data::ModuleDynamicMajor::Blocked)
                    ) {
                        blocked.0.insert(id_str.to_string());
                        false
                    } else if blocked.0.contains(&id_str.to_string()) {
                        warn!("filtered out a bilibili space item as it was blocked and probobly a fans-only post", kv: { id: = id_str });
                        false
                    } else {
                        true
                    }
                })
                .unwrap_or(true)
        })
        .filter_map(|item| {
            parse_item(&item, None)
                .inspect_err(|err| error!("failed to deserialize item", kv: { err:, item:? }))
                .ok()
        })
        .collect();

    Ok(Posts(items))
}

const ENDPOINT_URL: &str = "https://api.bilibili.com/x/polymer/web-dynamic/v1/feed/space";

async fn fetch_space(user_id: u64, cookies: Option<Cow<'_, str>>) -> anyhow::Result<(u32, String)> {
    let Some(cookies) = cookies else {
        return fetch_space_via_headless(user_id).await;
    };

    const FEATURES: &[&str] = &[
        "itemOpusStyle",
        "listOnlyfans",
        "opusBigCover",
        "onlyfansVote",
        "forwardListHidden",
        "decorationCard",
        "commentsNewVersion",
        "onlyfansAssetsV2",
        "ugcDelete",
        "onlyfansQaCard",
    ];
    let mut url = Url::from_str(ENDPOINT_URL)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("host_mid", &user_id.to_string());
        query.append_pair("platform", "web");
        query.append_pair("features", &FEATURES.join(","));
    }
    let resp = bilibili_request_builder()?
        .get(url.as_ref())
        .header(COOKIE, &*cookies)
        .send()
        .await
        .map_err(|err| anyhow!("failed to send request with cookies: {err}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|err| anyhow!("failed to read response text for bilibili: {err}"))?;
    Ok((status.as_u16() as u32, text))
}

async fn fetch_space_via_headless(user_id: u64) -> anyhow::Result<(u32, String)> {
    // Okay, I gave up on cracking the auth process
    use headless_chrome::{Browser, LaunchOptionsBuilder};

    let browser = Browser::new(
        LaunchOptionsBuilder::default()
            // https://github.com/rust-headless-chrome/rust-headless-chrome/issues/267
            .sandbox(false)
            .build()?,
    )?;

    let tab = browser.new_tab()?;
    let body_res = Arc::new(StdMutex::new(None));
    tab.register_response_handling(
        "",
        Box::new({
            let body_res = Arc::clone(&body_res);
            move |event, fetch_body| {
                if event.response.url.starts_with(ENDPOINT_URL) {
                    *body_res.lock().unwrap() = Some((event.response.status, fetch_body()));
                }
            }
        }),
    )?;

    // To Bilibili Dev:
    //
    // If you are seeing this, please let me know an appropriate rate to request via
    // email. This project is not intended to be a bad thing, just for personal use.
    //
    tab.set_user_agent(&prop::UserAgent::Mocked.as_str(), None, None)?;
    tab.navigate_to(&format!("https://space.bilibili.com/{user_id}/dynamic"))?;
    tab.wait_until_navigated()?;

    let mut body_res = body_res.lock().unwrap();
    let (status, body) = body_res
        .take()
        .ok_or_else(|| anyhow!("headless browser did not catch the expected response"))?;
    let body = body.map_err(|err| anyhow!("headless browser failed to fetch the body: {err}"))?;
    ensure!(
        !body.base_64_encoded,
        "headless browser returned a base64 encoded body"
    );

    Ok((status, body.body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deser() {
        let mut blocked = BlockedPostIds(HashSet::new());

        let history = fetch_space_history(8047632, &mut blocked, None)
            .await
            .unwrap();
        assert!(
            history
                .0
                .iter()
                .all(|post| !post.urls.major().as_clickable().unwrap().url.is_empty())
        );
        assert!(history.0.iter().all(|post| !post.content.is_empty()));

        let history = fetch_space_history(178362496, &mut blocked, None)
            .await
            .unwrap();
        assert!(
            history
                .0
                .iter()
                .all(|post| !post.urls.major().as_clickable().unwrap().url.is_empty())
        );
        assert!(history.0.iter().all(|post| !post.content.is_empty()));
    }
}
