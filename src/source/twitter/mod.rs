mod request;

use std::{
    collections::{hash_map::Entry, HashMap},
    fmt,
    future::Future,
    pin::Pin,
};

use anyhow::{anyhow, bail};
use request::*;
use serde::Deserialize;
use spdlog::prelude::*;
use tokio::sync::Mutex;

use super::{FetcherTrait, PostUrl, RepostFrom, User};
use crate::{
    config::{self, AsSecretRef, Config},
    platform::{PlatformMetadata, PlatformTrait},
    secret_enum,
    source::{
        Post, PostAttachment, PostAttachmentImage, PostAttachmentVideo, PostUrlClickable, PostUrls,
        Posts, Status, StatusKind, StatusSource,
    },
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigGlobal {
    pub auth: ConfigCookies,
}

secret_enum! {
    #[derive(Clone, Debug, PartialEq, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ConfigCookies {
        Cookies(String),
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigParams {
    pub username: String,
}

impl ConfigParams {
    pub fn validate(&self, global: &config::PlatformGlobal) -> anyhow::Result<()> {
        match &global.twitter {
            Some(global_twitter) => {
                let secret = global_twitter.auth.as_secret_ref();
                secret.validate()?;
                TwitterCookies::new(secret.get_str()?)?;
                Ok(())
            }
            None => bail!("cookies in global are missing"),
        }
    }
}

impl fmt::Display for ConfigParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Twitter:{}", self.username)
    }
}

//

mod data {
    use super::*;

    mod wrapper {
        use super::*;

        #[derive(Clone, Debug, PartialEq, Deserialize)]
        pub struct Data<T> {
            pub data: T,
        }

        #[derive(Clone, Debug, PartialEq, Deserialize)]
        pub struct User<T> {
            pub user: T,
        }

        #[derive(Clone, Debug, PartialEq, Deserialize)]
        pub struct Result<T> {
            pub result: T,
        }
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    #[serde(tag = "__typename")]
    pub enum ResultTweet {
        Tweet(Tweet),
        TweetWithVisibilityResults { tweet: Tweet },
    }

    impl ResultTweet {
        pub fn into_tweet(self) -> Tweet {
            match self {
                Self::Tweet(tweet) => tweet,
                Self::TweetWithVisibilityResults { tweet } => tweet,
            }
        }
    }

    #[derive(Copy, Clone, Debug, PartialEq, Deserialize)]
    pub struct Indices(pub u64, pub u64);

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct ResponseDataUserResult<T>(wrapper::Data<wrapper::User<wrapper::Result<T>>>);

    impl<T> ResponseDataUserResult<T> {
        pub fn into_inner(self) -> T {
            self.0.data.user.result
        }
    }

    //

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct UserByScreenName {
        pub rest_id: String,
        pub legacy: UserByScreenNameLegacy,
    }

    impl From<UserByScreenName> for User {
        fn from(user: UserByScreenName) -> Self {
            Self {
                nickname: user.legacy.name,
                profile_url: format!("https://x.com/{}", user.legacy.screen_name),
                avatar_url: user.legacy.profile_image_url_https,
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct UserByScreenNameLegacy {
        pub description: String,
        pub location: String,
        pub name: String,
        pub profile_banner_url: Option<String>,
        pub profile_image_url_https: String, // Very small..
        pub screen_name: String,
    }

    //

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct UserTweets {
        pub timeline_v2: UserTweetsTimelineV2,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct UserTweetsTimelineV2 {
        pub timeline: UserTweetsTimeline,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct UserTweetsTimeline {
        pub instructions: Vec<TimelineInstruction>,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    #[serde(tag = "type")]
    pub enum TimelineInstruction {
        #[serde(rename = "TimelineClearCache")]
        ClearCache,
        #[serde(rename = "TimelinePinEntry")]
        PinEntry { entry: TimelineEntry },
        #[serde(rename = "TimelineAddEntries")]
        AddEntries { entries: Vec<TimelineEntry> },
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TimelineEntry {
        pub content: TimelineEntryContent,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    #[serde(tag = "entryType")]
    pub enum TimelineEntryContent {
        #[serde(rename = "TimelineTimelineItem")]
        Item(TimelineItem),
        // "Who to follow", "Self conversation", etc.
        #[serde(rename = "TimelineTimelineModule")]
        Module { items: Vec<TimelineModuleItem> },
        #[serde(rename = "TimelineTimelineCursor")]
        Cursor,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TimelineModuleItem {
        pub item: TimelineItem,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TimelineItem {
        #[serde(rename = "itemContent")]
        pub item_content: TimelineItemContent,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    #[serde(tag = "itemType")]
    pub enum TimelineItemContent {
        #[serde(rename = "TimelineTweet")]
        Tweet {
            tweet_results: wrapper::Result<ResultTweet>,
        },
        #[serde(rename = "TimelineUser")]
        User,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct Tweet {
        pub rest_id: String,
        pub core: TweetCore,
        pub card: Option<TweetCard>,
        pub quoted_status_result: Option<wrapper::Result<Box<ResultTweet>>>,
        pub legacy: TweetLegacy,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TweetCore {
        pub user_results: wrapper::Result<UserByScreenName>,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TweetCard {
        pub legacy: TweetCardLegacy,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TweetCardLegacy {
        pub binding_values: Vec<TweetCardKV>,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TweetCardKV {
        pub key: String,
        pub value: TweetCardValue,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    #[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
    pub enum TweetCardValue {
        Boolean { boolean_value: bool },
        String { string_value: String },
        Image { image_value: TweetCardImageValue },
        ImageColor,
        User,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TweetCardImageValue {
        pub height: u64,
        pub width: u64,
        pub url: String,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TweetLegacy {
        pub created_at: String,
        pub conversation_id_str: String,
        pub entities: TweetLegacyEntities,
        pub full_text: String,
        pub is_quote_status: bool,
        pub possibly_sensitive: Option<bool>, // TODO
        pub user_id_str: String,
        pub retweeted_status_result: Option<wrapper::Result<Box<ResultTweet>>>,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TweetLegacyEntities {
        pub media: Option<Vec<TweetLegacyEntityMedia>>,
        pub urls: Vec<TweetLegacyEntityUrl>,
        pub user_mentions: Vec<TweetLegacyEntityUserMention>,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TweetLegacyEntityMedia {
        pub indices: Indices,
        pub media_url_https: String, // Image URL, or one frame for Video or AnimatedGif
        pub url: String, // The part presented in `full_text` (https://t.co/), needs to be replaced
        #[serde(rename = "type")]
        pub kind: TweetLegacyEntityMediaKind,
        pub video_info: Option<TweetLegacyEntityMediaVideoInfo>, // Video or AnimatedGif URL
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum TweetLegacyEntityMediaKind {
        Photo,
        Video,
        AnimatedGif,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TweetLegacyEntityMediaVideoInfo {
        pub variants: Vec<TweetLegacyEntityMediaVideoInfoVariant>,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TweetLegacyEntityMediaVideoInfoVariant {
        pub bitrate: Option<u64>,
        pub content_type: String,
        pub url: String,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TweetLegacyEntityUrl {
        pub display_url: String,  // Displayed on web page, incomplete real URL
        pub expanded_url: String, // Complete real URL
        pub url: String, // The part presented in `full_text` (https://t.co/), needs to be replaced
        pub indices: Indices,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TweetLegacyEntityUserMention {
        pub name: String,
        pub screen_name: String,
        pub indices: Indices,
    }
}

//

pub struct Fetcher {
    params: ConfigParams,
    inner: FetcherInner,
}

impl PlatformTrait for Fetcher {
    fn metadata(&self) -> PlatformMetadata {
        PlatformMetadata {
            display_name: "Twitter",
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
        let cookies = Config::platform_global()
            .twitter
            .as_ref()
            .unwrap()
            .auth
            .as_secret_ref()
            .get_str()
            .unwrap();
        Self {
            params,
            inner: FetcherInner::new(TwitterCookies::new(cookies).unwrap()),
        }
    }

    async fn fetch_status_impl(&self) -> anyhow::Result<Status> {
        let posts = self.inner.user_tweets(&self.params.username).await?;

        Ok(Status::new(
            StatusKind::Posts(posts),
            StatusSource {
                platform: self.metadata(),
                user: None, // TODO: Implement it later if needed
            },
        ))
    }
}

struct FetcherInner {
    requester: TwitterRequester,
    users: Mutex<HashMap<String /* username */, data::UserByScreenName>>,
}

impl FetcherInner {
    fn new(cookies: TwitterCookies) -> Self {
        Self {
            requester: TwitterRequester::new(cookies),
            users: Mutex::new(HashMap::new()),
        }
    }

    async fn user_id(&self, username: impl AsRef<str>) -> anyhow::Result<String> {
        match self.users.lock().await.entry(username.as_ref().into()) {
            Entry::Occupied(entry) => Ok(entry.get().rest_id.clone()),
            Entry::Vacant(entry) => {
                let resp = self
                    .requester
                    .user_by_screen_name(username.as_ref())
                    .await?
                    .json::<data::ResponseDataUserResult<data::UserByScreenName>>()
                    .await
                    .map_err(|err| anyhow!("failed to deserialize UserByScreenName: {err}"))?;
                Ok(entry.insert(resp.into_inner()).rest_id.clone())
            }
        }
    }

    async fn user_tweets(&self, username: impl AsRef<str>) -> anyhow::Result<Posts> {
        let username = username.as_ref();
        let user_id = self
            .user_id(username)
            .await
            .map_err(|err| anyhow!("failed to fetch user id for '{username}': {err}"))?;

        let resp = self
            .requester
            .user_tweets(user_id)
            .await?
            .json::<data::ResponseDataUserResult<data::UserTweets>>()
            .await
            .map_err(|err| anyhow!("failed to deserialize UserTweets: {err}"))?;

        let posts = resp
            .into_inner()
            .timeline_v2
            .timeline
            .instructions
            .into_iter()
            .filter_map(|instruction| match instruction {
                data::TimelineInstruction::ClearCache => None,
                data::TimelineInstruction::PinEntry { entry } => Some(vec![entry]),
                data::TimelineInstruction::AddEntries { entries } => Some(entries),
            })
            .flatten()
            .filter_map(|entry| match entry.content {
                data::TimelineEntryContent::Item(item) => Some(vec![item]),
                data::TimelineEntryContent::Module { items } => {
                    Some(items.into_iter().map(|item| item.item).collect())
                }
                data::TimelineEntryContent::Cursor => None,
            })
            .flatten()
            .filter_map(|item| match item.item_content {
                data::TimelineItemContent::Tweet { tweet_results } => Some(tweet_results),
                data::TimelineItemContent::User => None,
            })
            .map(|result| result.result.into_tweet())
            .map(parse_tweet)
            .collect::<Vec<_>>();

        Ok(Posts(posts))
    }
}

fn parse_tweet(tweet: data::Tweet) -> Post {
    let content = if tweet.legacy.retweeted_status_result.is_none() {
        Some(replace_entities(
            tweet.legacy.full_text,
            &tweet.legacy.entities,
        ))
    } else {
        None
    };

    let urls = PostUrls::new(PostUrl::Clickable(PostUrlClickable {
        url: format!(
            "https://x.com/{}/status/{}",
            tweet.core.user_results.result.legacy.screen_name, tweet.rest_id
        ),
        display: "View Tweet".into(),
    }));

    let repost_from = if !tweet.legacy.is_quote_status {
        tweet.legacy.retweeted_status_result
    } else {
        tweet.quoted_status_result
    }
    .map(|result| RepostFrom::Recursion(Box::new(parse_tweet(result.result.into_tweet()))));

    let possibly_sensitive = tweet.legacy.possibly_sensitive.unwrap_or(false);

    let card_attachment = tweet.card.and_then(|card| {
        const IMAGE_KEYS: [&str; 3] = [
            "photo_image_full_size_original",
            "summary_photo_image_original",
            "thumbnail_image_original",
        ];

        let image = IMAGE_KEYS.into_iter().find_map(|key| {
            card.legacy
                .binding_values
                .iter()
                .find_map(|kv| (kv.key == key).then_some(&kv.value))
        });

        match image {
            Some(data::TweetCardValue::Image { image_value }) => {
                Some(PostAttachment::Image(PostAttachmentImage {
                    media_url: image_value.url.clone(),
                    has_spoiler: possibly_sensitive,
                }))
            }
            Some(_) => {
                critical!(
                    "type of image card mismatched! tweet: {:?}, card kv: {:?}",
                    urls.major(),
                    card.legacy.binding_values
                );
                None
            }
            None => {
                if card
                    .legacy
                    .binding_values
                    .iter()
                    .any(|kv| matches!(kv.value, data::TweetCardValue::Image { .. }))
                {
                    let rustfmt_bug =
                        "expected image key not found in card, but the card contains image.";
                    warn!(
                        "{rustfmt_bug} tweet: {:?}, card kv: {:?}",
                        urls.major(),
                        card.legacy.binding_values
                    );
                }
                None
            }
        }
    });

    let attachments = tweet
        .legacy
        .entities
        .media
        .unwrap_or_default()
        .into_iter()
        .map(|media| match media.kind {
            data::TweetLegacyEntityMediaKind::Photo => PostAttachment::Image(PostAttachmentImage {
                media_url: media.media_url_https,
                has_spoiler: possibly_sensitive,
            }),
            data::TweetLegacyEntityMediaKind::Video
            | data::TweetLegacyEntityMediaKind::AnimatedGif => {
                // TODO: Distinguish GIF?
                let video_info = media.video_info.and_then(|mut video_info| {
                    video_info.variants.sort_by(|lhs, rhs| {
                        rhs.bitrate.unwrap_or(0).cmp(&lhs.bitrate.unwrap_or(0))
                    });
                    video_info.variants.into_iter().next()
                });
                match video_info {
                    Some(video_info) => PostAttachment::Video(PostAttachmentVideo {
                        media_url: video_info.url,
                        has_spoiler: possibly_sensitive,
                    }),
                    None => PostAttachment::Image(PostAttachmentImage {
                        media_url: media.media_url_https,
                        has_spoiler: possibly_sensitive,
                    }),
                }
            }
        })
        .chain(card_attachment)
        .filter(|attachment| {
            if let Some(RepostFrom::Recursion(repost_from)) = &repost_from {
                let is_contained_in_repost = repost_from
                    .attachments_recursive()
                    .iter()
                    .any(|sub_attachment| *sub_attachment == attachment);
                !is_contained_in_repost
            } else {
                true
            }
        })
        .collect();

    Post {
        user: Some(tweet.core.user_results.result.into()),
        content: content.unwrap_or_else(|| "".into()),
        urls,
        repost_from,
        attachments,
    }
}

enum ReplaceKind<'a> {
    Url(&'a str),
    Media,
}

fn replace_entities(mut text: String, entities: &data::TweetLegacyEntities) -> String {
    // TODO: entities.user_mentions

    let mut media_entities = entities.media.iter().flatten().collect::<Vec<_>>();
    // Multiple media share the same indices, they are expected to be overlapped
    media_entities.dedup_by_key(|media| media.indices);

    // Check overlapping indices
    let mut indices = media_entities
        .into_iter()
        .map(|media| (ReplaceKind::Media, media.indices))
        .chain(
            entities
                .urls
                .iter()
                .map(|url| (ReplaceKind::Url(&url.expanded_url), url.indices)),
        )
        .map(|(entity, indices)| (entity, (indices.0 as usize, indices.1 as usize)))
        .collect::<Vec<_>>();
    if is_indices_overlap(
        &indices
            .iter()
            .map(|(_, indices)| *indices)
            .collect::<Vec<_>>(),
    ) {
        warn!("overlapping indices in tweet, give up replacing entities. text: '{text}', entities: {entities:?}");
        return text;
    }

    indices.sort_by(|lhs, rhs| rhs.1 .0.cmp(&lhs.1 .0));
    indices.into_iter().for_each(|(entity, (start, end))| {
        let byte_pos = |utf8_pos| text.char_indices().nth(utf8_pos).map(|(pos, _)| pos);
        let range = (byte_pos(start), byte_pos(end - 1));
        if range.0.is_none() || range.1.is_none() {
            let rustfmt_bug = "invalid indices in tweet, give up replacing entities.";
            warn!("{rustfmt_bug} text: '{text}', entities: {entities:?}");
            return;
        }
        let range = range.0.unwrap()..=range.1.unwrap();

        match entity {
            ReplaceKind::Url(url) => {
                text.replace_range(range, url);
            }
            ReplaceKind::Media => {
                text.replace_range(range, "");
            }
        }
    });

    text.trim().into()
}

fn is_indices_overlap<I: Copy + PartialOrd>(indices: &[(I, I)]) -> bool {
    indices.iter().enumerate().any(|(i, (start1, end1))| {
        indices
            .iter()
            .enumerate()
            .any(|(j, (start2, end2))| i != j && end1 > start2 && start1 < end2)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices() {
        assert!(!is_indices_overlap(&[(1, 2), (3, 4), (7, 9)]));
        assert!(!is_indices_overlap(&[(1, 2), (3, 4), (4, 5)]));
        assert!(!is_indices_overlap(&[(3, 4), (4, 5), (1, 2)]));
        assert!(is_indices_overlap(&[(1, 2), (3, 6), (4, 9)]));
        assert!(is_indices_overlap(&[(1, 2), (3, 9), (4, 6)]));
        assert!(is_indices_overlap(&[(3, 9), (4, 6), (1, 2)]));
    }

    #[tokio::test]
    async fn posts() {
        let fetcher =
            FetcherInner::new(TwitterCookies::new(env!("CLOSELY_TEST_TWITTER_COOKIES")).unwrap());

        let posts = fetcher.user_tweets("NASA").await.unwrap().0;
        assert!(posts.iter().any(|post| !post.attachments.is_empty()));
        assert!(posts.iter().all(|post| post
            .urls
            .major()
            .as_clickable()
            .is_some_and(|url| url.url.starts_with("https://x.com/NASA/status"))));
    }
}
