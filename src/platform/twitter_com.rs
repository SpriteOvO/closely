use std::{
    collections::HashMap,
    fmt,
    future::Future,
    io::{self, Write},
    marker::PhantomData,
    pin::Pin,
    str::FromStr,
};

use anyhow::{anyhow, bail};
use chrono::NaiveDateTime;
use oauth1::OAuthClientProvider;
use once_cell::sync::Lazy;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT_LANGUAGE, AUTHORIZATION};
use reqwest_oauth1 as oauth1;
use serde::Deserialize;
use serde_json::{self as json, json};
use serde_qs as qs;
use spdlog::prelude::*;
use tokio::sync::OnceCell;

use super::{Fetcher, StatusSourceUser};
use crate::{
    config::{PlatformGlobalTwitterCom, PlatformTwitterCom},
    platform::{
        PlatformName, Post, PostAttachment, PostAttachmentImage, Posts, Status, StatusKind,
        StatusSource,
    },
    prop, util,
};

#[derive(Debug)]
struct TwitterCom;

#[derive(Debug)]
struct NitterNet;

#[derive(Debug)]
struct IncompleteUrl<H>(String, PhantomData<H>);

impl<H, S: Into<String>> From<S> for IncompleteUrl<H> {
    fn from(value: S) -> Self {
        Self(value.into(), PhantomData)
    }
}

impl<H> IncompleteUrl<H> {
    #[allow(dead_code)]
    fn incomplete_url(&self) -> &str {
        &self.0
    }
}

impl IncompleteUrl<TwitterCom> {
    fn real_url(&self) -> String {
        format!("https://twitter.com{}", self.0)
    }
}

impl IncompleteUrl<NitterNet> {
    fn real_url(&self) -> String {
        format!("https://nitter.net{}", self.0)
    }
}

#[derive(Debug)]
struct TwitterStatus {
    timeline: Vec<Tweet>,
    fullname: String,
}

#[derive(Debug)]
struct Tweet {
    url: IncompleteUrl<TwitterCom>,
    is_retweet: bool,
    is_quote: bool,
    #[allow(dead_code)]
    is_pinned: bool,
    date: NaiveDateTime,
    content: String,
    attachments: Vec<Attachment>,
}

#[derive(Debug)]
enum Attachment {
    Image(Image),
    Video(Video),
}

#[derive(Debug)]
struct Image {
    url: IncompleteUrl<NitterNet>,
}

#[derive(Debug)]
struct Video {
    preview_image_url: IncompleteUrl<NitterNet>,
}

pub struct TwitterComFetcher {
    params: PlatformTwitterCom,
}

impl Fetcher for TwitterComFetcher {
    fn fetch_status(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<Status>> + Send + '_>> {
        Box::pin(self.fetch_status_impl())
    }
}

impl fmt::Display for TwitterComFetcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.params)
    }
}

impl TwitterComFetcher {
    pub fn new(params: PlatformTwitterCom) -> Self {
        Self { params }
    }

    async fn fetch_status_impl(&self) -> anyhow::Result<Status> {
        let status = fetch_twitter_status(&self.params.username).await?;

        let posts = status
            .timeline
            .into_iter()
            .map(|tweet| Post {
                content: tweet.content,
                url: tweet.url.real_url(),
                is_repost: tweet.is_retweet,
                is_quote: tweet.is_quote,
                attachments: tweet
                    .attachments
                    .into_iter()
                    .map(|attachment| match attachment {
                        Attachment::Image(image) => PostAttachment::Image(PostAttachmentImage {
                            media_url: image.url.real_url(),
                        }),
                        // For now, we have no way to get the URL of the video, so we convert the
                        // preview image of the video into an image attachment.
                        //
                        // TODO: Add an overlay on the preview image to indicate that it's a video.
                        Attachment::Video(video) => PostAttachment::Image(PostAttachmentImage {
                            media_url: video.preview_image_url.real_url(),
                        }),
                    })
                    .collect(),
            })
            .collect();

        Ok(Status {
            kind: StatusKind::Posts(Posts(posts)),
            source: StatusSource {
                platform_name: PlatformName::TwitterCom,
                user: Some(StatusSourceUser {
                    display_name: status.fullname,
                    profile_url: format!("https://twitter.com/{}", self.params.username),
                }),
            },
        })
    }
}

const API_ACTIVATE_GUEST_TOKEN: &str = "https://api.twitter.com/1.1/guest/activate.json";
const API_TASK: &str = "https://api.twitter.com/1.1/onboarding/task.json";

const API_GRAPHQL_TIMELINE: &str =
    "https://api.twitter.com/graphql/3JNH4e9dq1BifLxAa3UMWg/UserWithProfileTweetsQueryV2";
const API_GRAPHQL_USER_BY_SCREEN_NAME: &str =
    "https://api.twitter.com/graphql/u7wQyGi6oExe8_TRWGMq4Q/UserResultByScreenNameQuery";

const AUTH_BEARER: &str = "Bearer AAAAAAAAAAAAAAAAAAAAAFXzAwAAAAAAMHCxpeSDG1gLNLghVe8d74hl6k4%3DRUMF4xAQLsbeBhTSRrCiQpJtxoGWeyHrDb5te2jpGskWDFW82F";
const OAUTH1_CONSUMER_KEY: &str = "3nVuSoBZnx6U4vzUxf5w";
const OAUTH1_CONSUMER_SECRET: &str = "Bcs59EFbbsdF6Sl9Ng71smgStWEGwXXKSjYvPVt7qys";

#[rustfmt::skip]
static FEATURES: Lazy<HashMap<&'static str, bool>> = Lazy::new(|| {
    HashMap::from_iter([
        ("android_graphql_skip_api_media_color_palette", false),
        ("blue_business_profile_image_shape_enabled", false),
        ("creator_subscriptions_subscription_count_enabled", false),
        ("creator_subscriptions_tweet_preview_api_enabled", true),
        ("freedom_of_speech_not_reach_fetch_enabled", false),
        ("longform_notetweets_consumption_enabled", true),
        ("longform_notetweets_inline_media_enabled", false),
        ("longform_notetweets_rich_text_read_enabled", false),
        ("subscriptions_verification_info_enabled", true),
        ("super_follow_badge_privacy_enabled", false),
        ("super_follow_exclusive_tweet_notifications_enabled", false),
        ("super_follow_tweet_api_enabled", false),
        ("super_follow_user_api_enabled", false),
        ("tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled", false),
        ("tweetypie_unmention_optimization_enabled", false),
        ("unified_cards_ad_metadata_container_dynamic_card_content_query_enabled", false),
        ("withQuickPromoteEligibilityTweetFields", true),
    ])
});

async fn graphql_get(endpoint: impl AsRef<str>) -> anyhow::Result<()> {
    let auth = AUTH.get().expect("Twitter not logged in");
    let secrets = oauth1::Secrets::new(OAUTH1_CONSUMER_KEY, OAUTH1_CONSUMER_SECRET)
        .token(&auth.oauth_token, &auth.oauth_token_secret);

    let resp = reqwest::Client::new()
        .oauth1(secrets)
        .get(format!("{}?{}", endpoint, qs::to_string(&*FEATURES)?))
        .send()
        .await
        .map_err(|err| anyhow!("failed to send request: {err}"))?;

    let status = resp.status();
    if !status.is_success() {
        bail!("response status is not success: {:?}", resp);
    }

    let text = resp
        .text()
        .await
        .map_err(|err| anyhow!("failed to obtain text from response: {err}"))?;

    todo!("{}", text)
}

async fn user_id_from_screen_name(screen_name: impl AsRef<str>) -> u64 {
    //
}

async fn fetch_twitter_status(username: impl AsRef<str>) -> anyhow::Result<TwitterStatus> {
    graphql_get(API_GRAPHQL_TIMELINE).await?;
}

#[derive(Deserialize)]
struct Auth {
    oauth_token: String,
    oauth_token_secret: String,
    user: AuthUser,
}

#[derive(Deserialize)]
struct AuthUser {
    name: String,
    screen_name: String,
}

pub async fn twitter_login(
    username: impl AsRef<str>,
    password: impl AsRef<str>,
) -> anyhow::Result<()> {
    let (username, password) = (username.as_ref(), password.as_ref());

    let resp = reqwest::Client::new()
        .post(API_ACTIVATE_GUEST_TOKEN)
        .header(AUTHORIZATION, AUTH_BEARER)
        .send()
        .await
        .map_err(|err| anyhow!("failed to request guest token: {err}"))?;
    if !resp.status().is_success() {
        bail!("response status of 'guest token' is not success: {resp:?}");
    }

    #[derive(Deserialize)]
    struct GuestTokenResp {
        guest_token: String,
    }

    let guest_token = resp
        .json::<GuestTokenResp>()
        .await
        .map_err(|err| anyhow!("failed to deserialize from 'guest token' response: {err}"))?
        .guest_token;

    trace!("guest_token: {guest_token}");

    // https://github.com/seanmonstar/reqwest/issues/402
    let header_name = |name: &'static str| HeaderName::from_str(&name.to_lowercase()).unwrap();

    let client = reqwest::ClientBuilder::new()
        .user_agent("TwitterAndroid/10.21.0-release.0 (310210000-r-0) ONEPLUS+A3010/9 (OnePlus;ONEPLUS+A3010;OnePlus;OnePlus3;0;;1;2016)")
        .default_headers(HeaderMap::from_iter([
            (AUTHORIZATION, (|| { let mut value = HeaderValue::from_static(AUTH_BEARER); value.set_sensitive(true); value })()),
            (header_name("X-Twitter-API-Version"), HeaderValue::from_static("5")),
            (header_name("X-Twitter-Client"), HeaderValue::from_static("TwitterAndroid")),
            (header_name("X-Twitter-Client-Version"), HeaderValue::from_static("10.21.0-release.0")),
            (header_name("OS-Version"), HeaderValue::from_static("28")),
            (header_name("System-User-Agent"), HeaderValue::from_static("Dalvik/2.1.0 (Linux; U; Android 9; ONEPLUS A3010 Build/PKQ1.181203.001)")),
            (header_name("X-Twitter-Active-User"), HeaderValue::from_static("yes")),
            (header_name("X-Guest-Token"), HeaderValue::from_str(&guest_token)?),
        ]))
        .build()?;

    // Task1

    let resp = client
        .post(API_TASK)
        .query(&[
            ("flow_name", "login"),
            ("api_version", "1"),
            ("known_device_token", ""),
            ("sim_country_code", "us"),
        ])
        .json(&json!({
            "flow_token": json::Value::Null,
            "input_flow_data": {
                "country_code": json::Value::Null,
                "flow_context": {
                    "referrer_context": {
                        "referral_details": "utm_source=google-play&utm_medium=organic",
                        "referrer_url": ""
                    },
                    "start_location": {
                        "location": "deeplink"
                    }
                },
                "requested_variant": json::Value::Null,
                "target_user_id": 0
            }
        }))
        .send()
        .await
        .map_err(|err| anyhow!("failed to request task1: {err}"))?;
    if !resp.status().is_success() {
        bail!("response status of 'task1' is not success: {resp:?}");
    }

    #[derive(Deserialize)]
    struct TaskResp {
        flow_token: String,
        subtasks: Vec<json::Value>,
    }

    let att = resp
        .headers()
        .get("att")
        .ok_or_else(|| anyhow!("header 'att' not found in task1 response"))?
        .clone();

    let flow_token = resp
        .json::<TaskResp>()
        .await
        .map_err(|err| anyhow!("failed to deserialize from 'task1' response: {err}"))?
        .flow_token;

    // Task2

    let resp = client
        .post(API_TASK)
        .header("att", att.clone())
        .json(&json!({
            "flow_token": flow_token,
            "subtask_inputs": [
                {
                    "enter_text": {
                        "suggestion_id": json::Value::Null,
                        "text": username,
                        "link": "next_link"
                    },
                    "subtask_id": "LoginEnterUserIdentifier"
                }
            ]
        }))
        .send()
        .await
        .map_err(|err| anyhow!("failed to request task2: {err}"))?;
    if !resp.status().is_success() {
        bail!("response status of 'task2' is not success: {resp:?}");
    }

    let flow_token = resp
        .json::<TaskResp>()
        .await
        .map_err(|err| anyhow!("failed to deserialize from 'task2' response: {err}"))?
        .flow_token;

    // Task3

    let resp = client
        .post(API_TASK)
        .header("att", att.clone())
        .json(&json!({
            "flow_token": flow_token,
            "subtask_inputs": [
                {
                    "enter_password": {
                        "password": password,
                        "link": "next_link"
                    },
                    "subtask_id": "LoginEnterPassword"
                }
            ]
        }))
        .send()
        .await
        .map_err(|err| anyhow!("failed to request task3: {err}"))?;
    if !resp.status().is_success() {
        bail!("response status of 'task3' is not success: {resp:?}");
    }

    let flow_token = resp
        .json::<TaskResp>()
        .await
        .map_err(|err| anyhow!("failed to deserialize from 'task3' response: {err}"))?
        .flow_token;

    // Task4

    let resp = client
        .post(API_TASK)
        .header("att", att.clone())
        .json(&json!({
            "flow_token": flow_token,
            "subtask_inputs": [
                {
                    "check_logged_in_account": {
                        "link": "AccountDuplicationCheck_false"
                    },
                    "subtask_id": "AccountDuplicationCheck"
                }
            ]
        }))
        .send()
        .await
        .map_err(|err| anyhow!("failed to request task4: {err}"))?;
    if !resp.status().is_success() {
        bail!("response status of 'task4' is not success: {resp:?}");
    }

    let mut resp = resp
        .json::<TaskResp>()
        .await
        .map_err(|err| anyhow!("failed to deserialize from 'task3' response: {err}"))?;
    let flow_token = resp.flow_token;

    let enter_text = resp
        .subtasks
        .iter_mut()
        .find_map(|subtask| subtask.as_object_mut().unwrap().remove("enter_text"));

    if let Some(enter_text) = enter_text {
        #[derive(Deserialize)]
        struct EnterText {
            hint_text: String,
        }
        let enter_text = json::from_value::<EnterText>(enter_text)?;

        print!("Twiiter is requesting '{}': ", enter_text.hint_text);

        let input = util::read_input()?;

        // Task5

        let resp_inner = client
            .post(API_TASK)
            .header("att", att)
            .json(&json!({
                "flow_token": flow_token,
                "subtask_inputs": [
                    {
                        "enter_text": {
                            "suggestion_id": json::Value::Null,
                            "text": input,
                            "link": "next_link",
                        },
                        "subtask_id": "LoginAcid",
                    }
                ]
            }))
            .send()
            .await
            .map_err(|err| anyhow!("failed to request task5: {err}"))?;
        if !resp_inner.status().is_success() {
            bail!(
                "response status of 'task5' is not success: {}",
                resp_inner.text().await.unwrap()
            );
        }

        resp = resp_inner
            .json::<TaskResp>()
            .await
            .map_err(|err| anyhow!("failed to deserialize from 'task3' response: {err}"))?
    };

    let auth = resp
        .subtasks
        .into_iter()
        .find_map(|subtask| {
            // TODO: Potential upstream contribution https://github.com/serde-rs/json/issues/852
            let subtask: Option<json::Map<String, json::Value>> = match subtask {
                json::Value::Object(subtask) => Some(subtask),
                _ => None,
            };
            subtask.and_then(|mut subtask| subtask.remove("open_account"))
        })
        .ok_or_else(|| anyhow!("auth not found in response"))?;

    let auth = json::from_value(auth)?;
    AUTH.set(auth)
        .map_err(|_| anyhow!("twitter auth already set"))?;

    Ok(())
}

static AUTH: OnceCell<Auth> = OnceCell::const_new();

pub async fn init_from_config(config: &PlatformGlobalTwitterCom) -> anyhow::Result<()> {
    twitter_login(&config.auth.username, config.auth.password()?).await
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    #[tokio::test]
    async fn timeline() {
        twitter_login(
            env!("CLOSELY_TEST_TWITTER_USERNAME"),
            env!("CLOSELY_TEST_TWITTER_PASSWORD"),
        )
        .await
        .unwrap();

        let year_2024 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let status = fetch_twitter_status("nasa").await.unwrap();

        assert_eq!(status.fullname, "NASA");

        let timeline_iter = || status.timeline.iter();

        assert!(timeline_iter().all(|tweet| tweet.date.date() > year_2024));
        assert!(timeline_iter().all(|tweet| !tweet.content.is_empty()));
        assert!(timeline_iter().any(|tweet| !tweet.attachments.is_empty()));
        assert!(timeline_iter().any(|tweet| tweet.url.incomplete_url().contains("/NASA/")));
    }
}
