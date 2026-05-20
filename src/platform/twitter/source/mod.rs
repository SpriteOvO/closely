pub mod post;
pub mod reply;

use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    sync::{LazyLock, Mutex as StdMutex},
};

use anyhow::anyhow;
use chrono::DateTime;
use futures::future::{OptionFuture, join_all};
use serde::Deserialize;
use spdlog::prelude::*;
use tokio::sync::Mutex;

use super::request::{TwitterCookies, TwitterRequester};
use crate::{
    config::{AccountRef, Config, ContextualValidator},
    source::{
        Post, PostAttachment, PostAttachmentImage, PostAttachmentVideo, PostContent, PostUrl,
        PostUrlClickable, PostUrls, Posts, RepostFrom, User,
    },
};

pub(crate) const TWITTER_IMAGE_URL_END_TAG: &str = ":orig";

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

        #[derive(Clone, Debug, PartialEq, Deserialize)]
        #[serde(untagged, deny_unknown_fields)]
        pub enum MaybeEmpty<T> {
            Empty {},
            Value(T),
        }

        impl<T> MaybeEmpty<T> {
            pub fn into_option(self) -> Option<T> {
                match self {
                    Self::Empty {} => None,
                    Self::Value(value) => Some(value),
                }
            }
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

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct ResponseData<T>(wrapper::Data<T>);

    impl<T> ResponseData<T> {
        pub fn into_inner(self) -> T {
            self.0.data
        }
    }

    //

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct UserByScreenName {
        pub rest_id: String,
        pub avatar: Avatar,
        pub core: UserByScreenNameCore,
        pub legacy: UserByScreenNameLegacy,
        pub location: Location,
    }

    impl From<UserByScreenName> for User {
        fn from(user: UserByScreenName) -> Self {
            Self {
                nickname: user.core.name,
                profile_url: format!("https://x.com/{}", user.core.screen_name),
                avatar_url: Some(user.avatar.image_url),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct UserByScreenNameCore {
        pub created_at: String,
        pub name: String,
        pub screen_name: String,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct Avatar {
        pub image_url: String,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct UserByScreenNameLegacy {
        pub description: String,
        pub pinned_tweet_ids_str: Option<Vec<String>>,
        pub profile_banner_url: Option<String>,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct Location {
        pub location: String,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TweetResult {
        #[serde(rename = "tweetResult")]
        pub tweet_result: wrapper::Result<ResultTweet>,
    }

    //

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct UserTweets {
        pub timeline: UserTweetsTimelineV2,
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
        #[serde(rename = "entryId")]
        pub entry_id: String,
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
            tweet_results: wrapper::Result<Box<ResultTweet>>,
        },
        #[serde(rename = "TimelineUser")]
        User,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct Tweet {
        pub rest_id: String,
        pub core: TweetCore,
        pub card: Option<TweetCard>,
        pub quoted_status_result: Option<wrapper::MaybeEmpty<wrapper::Result<Box<ResultTweet>>>>,
        pub legacy: TweetLegacy,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TweetCore {
        pub user_results: wrapper::Result<UserByScreenName>,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TweetCard {
        pub legacy: Option<TweetCardLegacy>,
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
        pub in_reply_to_status_id_str: Option<String>,
        pub is_quote_status: bool,
        pub possibly_sensitive: Option<bool>, // TODO
        pub user_id_str: String,
        pub retweeted_status_result: Option<wrapper::Result<Box<ResultTweet>>>,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TweetLegacyEntities {
        pub media: Option<Vec<TweetLegacyEntityMedia>>,
        pub url: Option<TweetLegacyEntitiesUrl>,
        pub user_mentions: Option<Vec<TweetLegacyEntityUserMention>>,
    }

    impl TweetLegacyEntities {
        pub fn urls(&self) -> impl Iterator<Item = &TweetLegacyEntityUrl> {
            self.url.iter().flat_map(|url| url.urls.iter())
        }
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
    pub struct TweetLegacyEntitiesUrl {
        pub urls: Vec<TweetLegacyEntityUrl>,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub struct TweetLegacyEntityUrl {
        pub display_url: String, // Displayed on web page, incomplete real URL
        pub expanded_url: Option<String>, // Complete real URL
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

pub(crate) fn validate_actor(actor: &AccountRef) -> anyhow::Result<()> {
    actor.validate(
        &Config::global()
            .platform()
            .twitter
            .as_ref()
            .ok_or_else(|| anyhow!("Twitter in global is missing"))?
            .account,
    )
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

        let posts = join_all(
            resp.into_inner()
                .timeline
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
                .map(|tweet| self.parse_tweet(tweet, true)),
        )
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

        Ok(Posts(posts))
    }

    async fn user_replies(&self, username: impl AsRef<str>) -> anyhow::Result<Posts> {
        let username = username.as_ref();
        let user_id = self
            .user_id(username)
            .await
            .map_err(|err| anyhow!("failed to fetch user id for '{username}': {err}"))?;

        let resp = self
            .requester
            .user_tweets_and_replies(user_id)
            .await?
            .json::<data::ResponseDataUserResult<data::UserTweets>>()
            .await
            .map_err(|err| anyhow!("failed to deserialize UserTweets for replies: {err}"))?;

        let conversations = join_all(
            resp.into_inner()
                .timeline
                .timeline
                .instructions
                .into_iter()
                .filter_map(|instruction| match instruction {
                    data::TimelineInstruction::ClearCache => None,
                    data::TimelineInstruction::PinEntry { entry } => Some(vec![entry]),
                    data::TimelineInstruction::AddEntries { entries } => Some(entries),
                })
                .flatten()
                .filter(|entry| entry.entry_id.starts_with("profile-conversation-"))
                .filter_map(|entry| match entry.content {
                    data::TimelineEntryContent::Item(item) => Some(vec![item]),
                    data::TimelineEntryContent::Module { items } => {
                        Some(items.into_iter().map(|item| item.item).collect())
                    }
                    data::TimelineEntryContent::Cursor => None,
                })
                .map(|items| {
                    join_all(
                        items
                            .into_iter()
                            .filter_map(|item| match item.item_content {
                                data::TimelineItemContent::Tweet { tweet_results } => Some(tweet_results),
                                data::TimelineItemContent::User => None,
                            })
                            .map(|result| result.result.into_tweet())
                            .map(|tweet| self.parse_tweet(tweet, false)),
                    )
                }),
        )
        .await
        .into_iter()
        .map(|conversation| conversation.into_iter().collect())
        .collect::<Result<Vec<Vec<_>>, _>>()?
        .into_iter()
        .filter_map(|conversation| {
            // Convert conversation
            // from: [ a ,  b ,  c ,  d ]
            //   to:   d -> c -> b -> a
            let mut conversations = conversation.into_iter().rev();
            let mut merged = conversations.next()?;
            let mut encountered_unexpected = false;
            conversations.fold(&mut merged, |last, reply| {
                encountered_unexpected |= last.repost_from.is_some();
                last.repost_from = Some(RepostFrom::new_reply(reply));
                &mut *last.repost_from.as_mut().unwrap().post
            });
            if encountered_unexpected {
                // This should not happen.
                warn!("replied post already has an unexpected repost_from, overwriting it", kv: { url:? = merged.urls.major() });
            }
            Some(merged)
        })
        .collect();

        Ok(Posts(conversations))
    }

    async fn tweet_result_by_rest_id(&self, tweet_id: impl AsRef<str>) -> anyhow::Result<Post> {
        let tweet = self
            .requester
            .tweet_result_by_rest_id(tweet_id)
            .await?
            .json::<data::ResponseData<data::TweetResult>>()
            .await
            .map_err(|err| anyhow!("failed to deserialize TweetResult by id: {err}"))?
            .into_inner()
            .tweet_result
            .result
            .into_tweet();
        self.parse_tweet(tweet, true).await
    }

    async fn parse_tweet(&self, tweet: data::Tweet, query_reply: bool) -> anyhow::Result<Post> {
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
                tweet.core.user_results.result.core.screen_name, tweet.rest_id
            ),
            display: "View Tweet".into(),
        }));

        let mut repost_from = OptionFuture::from(
            if !tweet.legacy.is_quote_status {
                tweet.legacy.retweeted_status_result
            } else {
                tweet.quoted_status_result.and_then(|q| q.into_option())
            }
            .map(|result| Box::pin(self.parse_tweet(result.result.into_tweet(), query_reply))),
        )
        .await
        .transpose()?
        .map(RepostFrom::new_quote);

        // Not a quote, but a reply?
        if query_reply && repost_from.is_none() {
            repost_from = OptionFuture::from(
                tweet
                    .legacy
                    .in_reply_to_status_id_str
                    .map(|tweet_id| Box::pin(self.tweet_result_by_rest_id(tweet_id))),
            )
            .await
            .transpose()?
            .map(RepostFrom::new_reply);
        }

        let possibly_sensitive = tweet.legacy.possibly_sensitive.unwrap_or(false);

        let card_attachment = tweet.card.and_then(|card| card.legacy).and_then(|legacy| {
            const IMAGE_KEYS: [&str; 3] = [
                "photo_image_full_size_original",
                "summary_photo_image_original",
                "thumbnail_image_original",
            ];

            let image = IMAGE_KEYS.into_iter().find_map(|key| {
                legacy
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
                        "type of image card mismatched!",
                        kv: { tweet:? = urls.major(), card_kv:? = legacy.binding_values }
                    );
                    None
                }
                None => {
                    if legacy
                        .binding_values
                        .iter()
                        .any(|kv| matches!(kv.value, data::TweetCardValue::Image { .. }))
                    {
                        // TODO: Make it more general for using in other places
                        static REPORTED: LazyLock<StdMutex<HashSet<String>>> =
                            LazyLock::new(|| StdMutex::new(HashSet::new()));

                        if REPORTED
                            .lock()
                            .unwrap()
                            .insert(urls.major().unique_id().into())
                        {
                            warn!(
                                "expected image key not found in card, but the card contains image",
                                kv: { tweet:? = urls.major(), card_kv:? = legacy.binding_values }
                            );
                        }
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
                data::TweetLegacyEntityMediaKind::Photo => {
                    PostAttachment::Image(PostAttachmentImage {
                        media_url: format!("{}{TWITTER_IMAGE_URL_END_TAG}", media.media_url_https),
                        has_spoiler: possibly_sensitive,
                    })
                }
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
                if let Some(repost_from) = &repost_from {
                    !repost_from
                        .post
                        .attachments_recursive(true)
                        .contains(&attachment)
                } else {
                    true
                }
            })
            .collect();

        let time = DateTime::parse_from_str(&tweet.legacy.created_at, "%a %b %d %H:%M:%S %z %Y")
            .map_err(|err| {
                anyhow!(
                    "failed to parse tweet time '{}', err: {err}, urls={urls:?}",
                    tweet.legacy.created_at
                )
            })?
            .into();

        let is_pinned = tweet
            .core
            .user_results
            .result
            .legacy
            .pinned_tweet_ids_str
            .as_ref()
            .is_some_and(|ids| ids.contains(&tweet.rest_id));

        Ok(Post {
            user: tweet.core.user_results.result.into(),
            content: PostContent::plain(
                content
                    .unwrap_or_else(|| if repost_from.is_some() { "Retweet" } else { "" }.into()),
            ),
            event: None,
            urls,
            time,
            is_pinned,
            repost_from,
            attachments,
            prefer_treat_as_reply: !query_reply,
        })
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
        .chain(entities.urls().map(|url| {
            (
                ReplaceKind::Url(url.expanded_url.as_deref().unwrap_or(&url.url)),
                url.indices,
            )
        }))
        .map(|(entity, indices)| (entity, (indices.0 as usize, indices.1 as usize)))
        .collect::<Vec<_>>();
    if is_indices_overlap(
        &indices
            .iter()
            .map(|(_, indices)| *indices)
            .collect::<Vec<_>>(),
    ) {
        warn!("overlapping indices in tweet, give up replacing entities", kv: { text, entities:? });
        return text;
    }

    indices.sort_by(|lhs, rhs| rhs.1.0.cmp(&lhs.1.0));
    indices.into_iter().for_each(|(entity, (start, end))| {
        let byte_pos = |utf8_pos| text.char_indices().nth(utf8_pos).map(|(pos, _)| pos);
        let range = (byte_pos(start), byte_pos(end - 1));
        if range.0.is_none() || range.1.is_none() {
            warn!("invalid indices in tweet, give up replacing entities", kv: { text, entities:? });
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
        assert!(posts.iter().all(|post| {
            post.urls
                .major()
                .as_clickable()
                .is_some_and(|url| url.url.starts_with("https://x.com/NASA/status"))
        }));
    }
}
