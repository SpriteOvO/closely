use anyhow::{anyhow, ensure};
use reqwest::header::COOKIE;

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
        let url = format!("https://x.com/i/api/graphql/xmU6X_CKVnQ5lSrCbAmJsg/UserByScreenName?variables=%7B%22screen_name%22%3A%22{screen_name}%22%2C%22withSafetyModeUserFields%22%3Atrue%7D&features=%7B%22hidden_profile_subscriptions_enabled%22%3Atrue%2C%22rweb_tipjar_consumption_enabled%22%3Atrue%2C%22responsive_web_graphql_exclude_directive_enabled%22%3Atrue%2C%22verified_phone_label_enabled%22%3Afalse%2C%22subscriptions_verification_info_is_identity_verified_enabled%22%3Atrue%2C%22subscriptions_verification_info_verified_since_enabled%22%3Atrue%2C%22highlights_tweets_tab_ui_enabled%22%3Atrue%2C%22responsive_web_twitter_article_notes_tab_enabled%22%3Atrue%2C%22subscriptions_feature_can_gift_premium%22%3Afalse%2C%22creator_subscriptions_tweet_preview_api_enabled%22%3Atrue%2C%22responsive_web_graphql_skip_user_profile_image_extensions_enabled%22%3Afalse%2C%22responsive_web_graphql_timeline_navigation_enabled%22%3Atrue%7D&fieldToggles=%7B%22withAuxiliaryUserLabels%22%3Afalse%7D");
        self.request(url)
            .await
            .map_err(|err| anyhow!("failed to fetch user by screen name: {err}"))
    }

    pub async fn user_tweets(&self, user_id: impl AsRef<str>) -> anyhow::Result<reqwest::Response> {
        let user_id = user_id.as_ref();
        let url = format!("https://x.com/i/api/graphql/V7H0Ap3_Hh2FyS75OCDO3Q/UserTweets?variables=%7B%22userId%22%3A%22{user_id}%22%2C%22count%22%3A20%2C%22includePromotedContent%22%3Atrue%2C%22withQuickPromoteEligibilityTweetFields%22%3Atrue%2C%22withVoice%22%3Atrue%2C%22withV2Timeline%22%3Atrue%7D&features=%7B%22rweb_tipjar_consumption_enabled%22%3Atrue%2C%22responsive_web_graphql_exclude_directive_enabled%22%3Atrue%2C%22verified_phone_label_enabled%22%3Afalse%2C%22creator_subscriptions_tweet_preview_api_enabled%22%3Atrue%2C%22responsive_web_graphql_timeline_navigation_enabled%22%3Atrue%2C%22responsive_web_graphql_skip_user_profile_image_extensions_enabled%22%3Afalse%2C%22communities_web_enable_tweet_community_results_fetch%22%3Atrue%2C%22c9s_tweet_anatomy_moderator_badge_enabled%22%3Atrue%2C%22articles_preview_enabled%22%3Atrue%2C%22tweetypie_unmention_optimization_enabled%22%3Atrue%2C%22responsive_web_edit_tweet_api_enabled%22%3Atrue%2C%22graphql_is_translatable_rweb_tweet_is_translatable_enabled%22%3Atrue%2C%22view_counts_everywhere_api_enabled%22%3Atrue%2C%22longform_notetweets_consumption_enabled%22%3Atrue%2C%22responsive_web_twitter_article_tweet_consumption_enabled%22%3Atrue%2C%22tweet_awards_web_tipping_enabled%22%3Afalse%2C%22creator_subscriptions_quote_tweet_preview_enabled%22%3Afalse%2C%22freedom_of_speech_not_reach_fetch_enabled%22%3Atrue%2C%22standardized_nudges_misinfo%22%3Atrue%2C%22tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled%22%3Atrue%2C%22rweb_video_timestamps_enabled%22%3Atrue%2C%22longform_notetweets_rich_text_read_enabled%22%3Atrue%2C%22longform_notetweets_inline_media_enabled%22%3Atrue%2C%22responsive_web_enhance_cards_enabled%22%3Afalse%7D&fieldToggles=%7B%22withArticlePlainText%22%3Afalse%7D");
        self.request(url)
            .await
            .map_err(|err| anyhow!("failed to fetch user tweets: {err}"))
    }

    async fn request(&self, url: impl AsRef<str>) -> anyhow::Result<reqwest::Response> {
        let resp = reqwest::Client::new()
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
