use std::{
    str::FromStr,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use anyhow::{anyhow, ensure};
use headless_chrome::{
    browser::tab::RequestPausedDecision,
    protocol::cdp::{
        Fetch::{events::RequestPausedEvent, RequestPattern, RequestStage},
        Network::CookieParam,
    },
};
use reqwest::{header::COOKIE, Url};
use serde_json::{self as json, json};
use spdlog::prelude::*;
use tokio::{sync::Mutex, time::sleep};

use crate::{helper, prop};

pub struct TwitterCookies {
    pub raw: String,
    pub ct0: String,
    pub splitted: Vec<(String, String)>,
}

impl TwitterCookies {
    pub fn new(raw: impl Into<String>) -> anyhow::Result<Self> {
        let raw = raw.into();

        let splitted = raw
            .split(';')
            .filter_map(|cookie| {
                let mut kv = cookie.trim().split('=').map(|kv| kv.trim().to_owned());
                kv.next().and_then(|k| kv.next().map(|v| (k, v)))
            })
            .collect::<Vec<_>>();

        let ct0 = splitted
            .iter()
            .find_map(|(k, v)| (k == "ct0").then_some(v))
            .ok_or_else(|| anyhow!("cookie 'ct0' not found"))?
            .into();
        Ok(Self { raw, ct0, splitted })
    }
}

pub struct TwitterRequester {
    cookies: TwitterCookies,
    transaction_id: Mutex<Option<String>>,
}

impl TwitterRequester {
    pub fn new(cookies: TwitterCookies) -> Self {
        Self {
            cookies,
            transaction_id: Mutex::new(None),
        }
    }

    pub async fn user_by_screen_name(
        &self,
        screen_name: impl AsRef<str>,
    ) -> anyhow::Result<reqwest::Response> {
        let screen_name = screen_name.as_ref();

        let variables = json!({
            "screen_name": screen_name,
            "withGrokTranslatedBio": false
        });
        let features = json!({
            "hidden_profile_subscriptions_enabled": true,
            "payments_enabled": false,
            "rweb_xchat_enabled": false,
            "profile_label_improvements_pcf_label_in_post_enabled": true,
            "rweb_tipjar_consumption_enabled": true,
            "verified_phone_label_enabled": false,
            "subscriptions_verification_info_is_identity_verified_enabled": true,
            "subscriptions_verification_info_verified_since_enabled": true,
            "highlights_tweets_tab_ui_enabled": true,
            "responsive_web_twitter_article_notes_tab_enabled": true,
            "subscriptions_feature_can_gift_premium": true,
            "creator_subscriptions_tweet_preview_api_enabled": true,
            "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
            "responsive_web_graphql_timeline_navigation_enabled": true
        });
        let field_toggles = json!({
            "withAuxiliaryUserLabels": true
        });
        let mut url =
            Url::from_str("https://x.com/i/api/graphql/gEyDv8Fmv2BVTYIAf32nbA/UserByScreenName")?;
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
            "count": 40,
            "includePromotedContent": true,
            "withQuickPromoteEligibilityTweetFields": true,
            "withVoice": true,
        });
        let features = json!({
            "rweb_video_screen_enabled": false,
            "payments_enabled": false,
            "rweb_xchat_enabled": false,
            "profile_label_improvements_pcf_label_in_post_enabled": true,
            "rweb_tipjar_consumption_enabled": true,
            "verified_phone_label_enabled": false,
            "creator_subscriptions_tweet_preview_api_enabled": true,
            "responsive_web_graphql_timeline_navigation_enabled": true,
            "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
            "premium_content_api_read_enabled": false,
            "communities_web_enable_tweet_community_results_fetch": true,
            "c9s_tweet_anatomy_moderator_badge_enabled": true,
            "responsive_web_grok_analyze_button_fetch_trends_enabled": false,
            "responsive_web_grok_analyze_post_followups_enabled": true,
            "responsive_web_jetfuel_frame": true,
            "responsive_web_grok_share_attachment_enabled": true,
            "articles_preview_enabled": true,
            "responsive_web_edit_tweet_api_enabled": true,
            "graphql_is_translatable_rweb_tweet_is_translatable_enabled": true,
            "view_counts_everywhere_api_enabled": true,
            "longform_notetweets_consumption_enabled": true,
            "responsive_web_twitter_article_tweet_consumption_enabled": true,
            "tweet_awards_web_tipping_enabled": false,
            "responsive_web_grok_show_grok_translated_post": false,
            "responsive_web_grok_analysis_button_from_backend": true,
            "creator_subscriptions_quote_tweet_preview_enabled": false,
            "freedom_of_speech_not_reach_fetch_enabled": true,
            "standardized_nudges_misinfo": true,
            "tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled": true,
            "longform_notetweets_rich_text_read_enabled": true,
            "longform_notetweets_inline_media_enabled": true,
            "responsive_web_grok_image_annotation_enabled": true,
            "responsive_web_grok_imagine_annotation_enabled": true,
            "responsive_web_grok_community_note_auto_translation_is_enabled": false,
            "responsive_web_enhance_cards_enabled": false
        });
        let field_toggles = json!({
            "withArticlePlainText": false
        });
        let mut url =
            Url::from_str("https://x.com/i/api/graphql/BqvqNsqColIQbpX1_NmEwg/UserTweets")?;
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

    pub async fn user_tweets_and_replies(
        &self,
        user_id: impl AsRef<str>,
    ) -> anyhow::Result<reqwest::Response> {
        let user_id = user_id.as_ref();

        let variables = json!({
            "userId": user_id,
            "count": 20,
            "includePromotedContent": true,
            "withCommunity": true,
            "withVoice": true
        });
        let features = json!({
            "rweb_video_screen_enabled": false,
            "profile_label_improvements_pcf_label_in_post_enabled": true,
            "responsive_web_profile_redirect_enabled": false,
            "rweb_tipjar_consumption_enabled": true,
            "verified_phone_label_enabled": false,
            "creator_subscriptions_tweet_preview_api_enabled": true,
            "responsive_web_graphql_timeline_navigation_enabled": true,
            "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
            "premium_content_api_read_enabled": false,
            "communities_web_enable_tweet_community_results_fetch": true,
            "c9s_tweet_anatomy_moderator_badge_enabled": true,
            "responsive_web_grok_analyze_button_fetch_trends_enabled": false,
            "responsive_web_grok_analyze_post_followups_enabled": true,
            "responsive_web_jetfuel_frame": true,
            "responsive_web_grok_share_attachment_enabled": true,
            "responsive_web_grok_annotations_enabled": false,
            "articles_preview_enabled": true,
            "responsive_web_edit_tweet_api_enabled": true,
            "graphql_is_translatable_rweb_tweet_is_translatable_enabled": true,
            "view_counts_everywhere_api_enabled": true,
            "longform_notetweets_consumption_enabled": true,
            "responsive_web_twitter_article_tweet_consumption_enabled": true,
            "tweet_awards_web_tipping_enabled": false,
            "responsive_web_grok_show_grok_translated_post": true,
            "responsive_web_grok_analysis_button_from_backend": true,
            "post_ctas_fetch_enabled": true,
            "creator_subscriptions_quote_tweet_preview_enabled": false,
            "freedom_of_speech_not_reach_fetch_enabled": true,
            "standardized_nudges_misinfo": true,
            "tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled": true,
            "longform_notetweets_rich_text_read_enabled": true,
            "longform_notetweets_inline_media_enabled": true,
            "responsive_web_grok_image_annotation_enabled": true,
            "responsive_web_grok_imagine_annotation_enabled": true,
            "responsive_web_grok_community_note_auto_translation_is_enabled": false,
            "responsive_web_enhance_cards_enabled": false,
        });
        let field_toggles = json!({
            "withArticlePlainText": false
        });
        let mut url = Url::from_str(
            "https://x.com/i/api/graphql/rUGgLrfxEz17FY2HSk2b6w/UserTweetsAndReplies",
        )?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("variables", &json::to_string(&variables)?);
            query.append_pair("features", &json::to_string(&features)?);
            query.append_pair("fieldToggles", &json::to_string(&field_toggles)?);
        }

        self.request_with_transaction_id(url)
            .await
            .map_err(|err| anyhow!("failed to fetch user tweets and replies: {err}"))
    }

    pub async fn tweet_result_by_rest_id(
        &self,
        tweet_id: impl AsRef<str>,
    ) -> anyhow::Result<reqwest::Response> {
        let tweet_id = tweet_id.as_ref();

        let variables = json!({
            "tweetId": tweet_id,
            "includePromotedContent": true,
            "withBirdwatchNotes": true,
            "withVoice": true,
            "withCommunity": true
        });
        let features = json!({
            "creator_subscriptions_tweet_preview_api_enabled": true,
            "premium_content_api_read_enabled": false,
            "communities_web_enable_tweet_community_results_fetch": true,
            "c9s_tweet_anatomy_moderator_badge_enabled": true,
            "responsive_web_grok_analyze_button_fetch_trends_enabled": false,
            "responsive_web_grok_analyze_post_followups_enabled": true,
            "responsive_web_jetfuel_frame": true,
            "responsive_web_grok_share_attachment_enabled": true,
            "articles_preview_enabled": true,
            "responsive_web_edit_tweet_api_enabled": true,
            "graphql_is_translatable_rweb_tweet_is_translatable_enabled": true,
            "view_counts_everywhere_api_enabled": true,
            "longform_notetweets_consumption_enabled": true,
            "responsive_web_twitter_article_tweet_consumption_enabled": true,
            "tweet_awards_web_tipping_enabled": false,
            "responsive_web_grok_show_grok_translated_post": true,
            "responsive_web_grok_analysis_button_from_backend": true,
            "creator_subscriptions_quote_tweet_preview_enabled": false,
            "freedom_of_speech_not_reach_fetch_enabled": true,
            "standardized_nudges_misinfo": true,
            "tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled": true,
            "longform_notetweets_rich_text_read_enabled": true,
            "longform_notetweets_inline_media_enabled": true,
            "payments_enabled": false,
            "profile_label_improvements_pcf_label_in_post_enabled": true,
            "responsive_web_profile_redirect_enabled": false,
            "rweb_tipjar_consumption_enabled": true,
            "verified_phone_label_enabled": false,
            "responsive_web_grok_image_annotation_enabled": true,
            "responsive_web_grok_imagine_annotation_enabled": true,
            "responsive_web_grok_community_note_auto_translation_is_enabled": false,
            "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
            "responsive_web_graphql_timeline_navigation_enabled": true,
            "responsive_web_enhance_cards_enabled": false
        });
        let field_toggles = json!({
            "withArticleRichContentState": true,
            "withArticlePlainText": false
        });
        let mut url = Url::from_str(
            "https://x.com/i/api/graphql/WvlrBJ2bz8AuwoszWyie8A/TweetResultByRestId",
        )?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("variables", &json::to_string(&variables)?);
            query.append_pair("features", &json::to_string(&features)?);
            query.append_pair("fieldToggles", &json::to_string(&field_toggles)?);
        }

        self.request(url)
            .await
            .map_err(|err| anyhow!("failed to fetch tweet result by rest id: {err}"))
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

    async fn request_with_transaction_id(
        &self,
        url: impl AsRef<str>,
    ) -> anyhow::Result<reqwest::Response> {
        let url = url.as_ref();

        let mut transaction_id = self.transaction_id.lock().await;
        if transaction_id.is_none() {
            *transaction_id = Some(self.get_x_client_transaction_id().await?);
        }

        let res = self
            .request_with_transaction_id_inner(url, transaction_id.as_deref().unwrap())
            .await;

        match res {
            Err(RequestError::Auth) => {
                warn!("Twitter 404 auth error, refreshing transaction id and retrying");
                // Retry once to get a new transaction id
                *transaction_id = Some(self.get_x_client_transaction_id().await?);
                let res = self
                    .request_with_transaction_id_inner(url, transaction_id.as_deref().unwrap())
                    .await;
                // Auth error again, clear the transaction id to force refresh next time.
                if matches!(res, Err(RequestError::Auth)) {
                    *transaction_id = None;
                }
                res
            }
            other => other,
        }
        .map_err(RequestError::anyway)
    }

    async fn request_with_transaction_id_inner(
        &self,
        url: impl AsRef<str>,
        transaction_id: &str,
    ) -> Result<reqwest::Response, RequestError> {
        let resp = helper::reqwest_client()?
            .get(url.as_ref())
            .bearer_auth(BEARER_TOKEN)
            .header(COOKIE, &self.cookies.raw)
            .header("x-csrf-token", &self.cookies.ct0)
            .header("x-client-transaction-id", transaction_id)
            .send()
            .await
            .map_err(|err| anyhow!("failed to send request for Twitter: {err}"))?;

        let status = resp.status();
        match status.as_u16() {
            404 => return Err(RequestError::Auth),
            _ => {
                if !status.is_success() {
                    return Err(RequestError::Other(anyhow!(
                        "response status from Twitter is not success: {resp:?}"
                    )));
                }
            }
        }
        Ok(resp)
    }

    async fn get_x_client_transaction_id(&self) -> anyhow::Result<String> {
        use headless_chrome::{Browser, LaunchOptionsBuilder};

        let browser = Browser::new(
            LaunchOptionsBuilder::default()
                // https://github.com/rust-headless-chrome/rust-headless-chrome/issues/267
                .sandbox(false)
                .build()?,
        )?;

        let tab = browser.new_tab()?;
        let id = Arc::new(StdMutex::new(None));
        tab.enable_fetch(
            Some(&[RequestPattern {
                url_pattern: None,
                resource_Type: None,
                request_stage: Some(RequestStage::Request),
            }]),
            None,
        )?;
        tab.enable_request_interception(Arc::new({
            let id = id.clone();
            move |_transport, _session_id, event: RequestPausedEvent| {
                let mut id = id.lock().unwrap();
                let url = event.params.request.url;
                if id.is_none()
                    && url.starts_with("https://x.com/i/api/graphql/")
                    && url.contains("/UserTweetsAndReplies")
                {
                    *id = event.params.request.headers.0.as_ref().and_then(|headers| {
                        headers["x-client-transaction-id"]
                            .as_str()
                            .map(ToString::to_string)
                    });
                }
                RequestPausedDecision::Continue(None)
            }
        }))?;
        tab.set_cookies(
            self.cookies
                .splitted
                .iter()
                .map(|(k, v)| CookieParam {
                    name: k.clone(),
                    value: v.clone(),
                    url: None,
                    domain: Some(".x.com".into()),
                    path: None,
                    secure: None,
                    http_only: None,
                    same_site: None,
                    expires: None,
                    priority: None,
                    same_party: None,
                    source_scheme: None,
                    source_port: None,
                    partition_key: None,
                })
                .collect(),
        )?;
        tab.set_user_agent(&prop::UserAgent::Mocked.as_str(), None, None)?;
        tab.navigate_to("https://x.com/NASA/with_replies")?;
        // Do not use `wait_until_navigated` because Twitter will never make the browser
        // report networkAlmostIdle.
        //
        // tab.wait_until_navigated()?;

        tokio::select! {
            _ = async {
                while id.lock().unwrap().is_none() {
                   sleep(Duration::from_millis(100)).await;
                }
            } => {}
            _ = sleep(Duration::from_secs(30)) => {}
        };

        let id = id.lock().unwrap().take().ok_or_else(|| {
            anyhow!("headless browser did not catch the expected header for Twitter")
        })?;
        Ok(id)
    }
}

const BEARER_TOKEN: &str = "AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";

enum RequestError {
    Auth,
    Other(anyhow::Error),
}

impl From<anyhow::Error> for RequestError {
    fn from(err: anyhow::Error) -> Self {
        RequestError::Other(err)
    }
}

impl RequestError {
    fn anyway(self) -> anyhow::Error {
        match self {
            RequestError::Auth => anyhow!("Twitter 404 auth error, already retried once"),
            RequestError::Other(err) => err,
        }
    }
}
