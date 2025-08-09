use std::str::FromStr;

use anyhow::{anyhow, ensure};
use reqwest::{header::COOKIE, Url};
use serde_json::{self as json, json};

use crate::helper;

pub struct TwitterCookies {
    pub raw: String,
    pub ct0: String,
}

impl TwitterCookies {
    pub fn new(raw: impl Into<String>) -> anyhow::Result<Self> {
        let raw = raw.into();
        let ct0 = raw
            .split(';')
            .map(|cookie| cookie.trim())
            .find_map(|cookie| {
                cookie
                    .starts_with("ct0=")
                    .then_some(cookie.trim_start_matches("ct0="))
            })
            .ok_or_else(|| anyhow!("cookie 'ct0' not found"))?
            .into();
        Ok(Self { raw, ct0 })
    }
}

pub struct TwitterRequester {
    cookies: TwitterCookies,
}

impl TwitterRequester {
    pub fn new(cookies: TwitterCookies) -> Self {
        Self { cookies }
    }

    pub async fn user_by_screen_name(
        &self,
        screen_name: impl AsRef<str>,
    ) -> anyhow::Result<reqwest::Response> {
        let screen_name = screen_name.as_ref();

        let variables = json!({
            "screen_name": screen_name,
            "withSafetyModeUserFields": true
        });
        let features = json!({
            "hidden_profile_subscriptions_enabled": true,
            "rweb_tipjar_consumption_enabled": true,
            "responsive_web_graphql_exclude_directive_enabled": true,
            "verified_phone_label_enabled": false,
            "subscriptions_verification_info_is_identity_verified_enabled": true,
            "subscriptions_verification_info_verified_since_enabled": true,
            "highlights_tweets_tab_ui_enabled": true,
            "responsive_web_twitter_article_notes_tab_enabled": true,
            "subscriptions_feature_can_gift_premium": false,
            "creator_subscriptions_tweet_preview_api_enabled": true,
            "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
            "responsive_web_graphql_timeline_navigation_enabled": true
        });
        let field_toggles = json!({
            "withAuxiliaryUserLabels": false
        });
        let mut url =
            Url::from_str("https://x.com/i/api/graphql/xmU6X_CKVnQ5lSrCbAmJsg/UserByScreenName")?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("variables", &json::to_string(&variables)?);
            query.append_pair("features", &json::to_string(&features)?);
            query.append_pair("fieldToggles", &json::to_string(&field_toggles)?);
        }

        self.request(url)
            .await
            .map_err(|err| anyhow!("failed to fetch user by screen name: {err}"))
    }

    pub async fn user_tweets(&self, user_id: impl AsRef<str>) -> anyhow::Result<reqwest::Response> {
        let user_id = user_id.as_ref();

        let variables = json!({
            "userId": user_id,
            "count": 20,
            "includePromotedContent": true,
            "withQuickPromoteEligibilityTweetFields": true,
            "withVoice": true,
            "withV2Timeline": true
        });
        let features = json!({
            "rweb_tipjar_consumption_enabled": true,
            "responsive_web_graphql_exclude_directive_enabled": true,
            "verified_phone_label_enabled": false,
            "creator_subscriptions_tweet_preview_api_enabled": true,
            "responsive_web_graphql_timeline_navigation_enabled": true,
            "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
            "communities_web_enable_tweet_community_results_fetch": true,
            "c9s_tweet_anatomy_moderator_badge_enabled": true,
            "articles_preview_enabled": true,
            "tweetypie_unmention_optimization_enabled": true,
            "responsive_web_edit_tweet_api_enabled": true,
            "graphql_is_translatable_rweb_tweet_is_translatable_enabled": true,
            "view_counts_everywhere_api_enabled": true,
            "longform_notetweets_consumption_enabled": true,
            "responsive_web_twitter_article_tweet_consumption_enabled": true,
            "tweet_awards_web_tipping_enabled": false,
            "creator_subscriptions_quote_tweet_preview_enabled": false,
            "freedom_of_speech_not_reach_fetch_enabled": true,
            "standardized_nudges_misinfo": true,
            "tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled": true,
            "rweb_video_timestamps_enabled": true,
            "longform_notetweets_rich_text_read_enabled": true,
            "longform_notetweets_inline_media_enabled": true,
            "responsive_web_enhance_cards_enabled": false
        });
        let field_toggles = json!({
            "withArticlePlainText": false
        });
        let mut url =
            Url::from_str("https://x.com/i/api/graphql/V7H0Ap3_Hh2FyS75OCDO3Q/UserTweets")?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("variables", &json::to_string(&variables)?);
            query.append_pair("features", &json::to_string(&features)?);
            query.append_pair("fieldToggles", &json::to_string(&field_toggles)?);
        }

        self.request(url)
            .await
            .map_err(|err| anyhow!("failed to fetch user tweets: {err}"))
    }

    async fn request(&self, url: impl AsRef<str>) -> anyhow::Result<reqwest::Response> {
        let resp = helper::reqwest_client()?
            .get(url.as_ref())
            .bearer_auth(BEARER_TOKEN)
            .header(COOKIE, &self.cookies.raw)
            .header("x-csrf-token", &self.cookies.ct0)
            .send()
            .await
            .map_err(|err| anyhow!("failed to send request for Twitter: {err}"))?;

        let status = resp.status();
        ensure!(
            status.is_success(),
            "response status from Twitter is not success: {resp:?}"
        );

        Ok(resp)
    }
}

const BEARER_TOKEN: &str = "AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";
