use std::{borrow::Cow, env, fmt, fmt::Write, future::Future, ops::Range, pin::Pin};

use anyhow::{anyhow, bail};
use serde::{de::IgnoredAny, Deserialize};
use serde_json::{self as json, json};
use spdlog::prelude::*;
use tokio::sync::Mutex;

use super::NotifierTrait;
use crate::{
    config::{self, Config},
    source::{
        LiveStatus, Notification, NotificationKind, Post, PostAttachment, PostsRef, RepostFrom,
        StatusSource,
    },
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigGlobal {
    #[serde(flatten)]
    pub token: ConfigToken,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigParams {
    #[serde(default)]
    pub notifications: config::Notifications,
    #[serde(flatten)]
    pub chat: ConfigChat,
    pub thread_id: Option<i64>,
    #[serde(flatten)]
    pub token: Option<ConfigToken>,
}

impl fmt::Display for ConfigParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "telegram:{}", self.chat)?;
        if let Some(thread_id) = self.thread_id {
            write!(f, ":({})", thread_id)?;
        }
        Ok(())
    }
}

impl config::Overridable for ConfigParams {
    type Override = ConfigOverride;

    fn override_into(self, new: Self::Override) -> Self
    where
        Self: Sized,
    {
        Self {
            notifications: match new.notifications {
                Some(notifications) => self.notifications.override_into(notifications),
                None => self.notifications,
            },
            chat: new.chat.unwrap_or(self.chat),
            thread_id: new.thread_id.or(self.thread_id),
            token: new.token.or(self.token),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigOverride {
    pub notifications: Option<config::NotificationsOverride>,
    #[serde(flatten)]
    pub chat: Option<ConfigChat>,
    pub thread_id: Option<i64>,
    #[serde(flatten)]
    token: Option<ConfigToken>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigToken {
    Token(String),
    TokenEnv(String),
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigChat {
    Id(i64),
    Username(String),
}

impl fmt::Display for ConfigChat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigChat::Id(id) => write!(f, "{}", id),
            ConfigChat::Username(username) => write!(f, "@{}", username),
        }
    }
}

impl ConfigToken {
    pub fn get(&self) -> anyhow::Result<Cow<str>> {
        match &self {
            Self::Token(token) => Ok(Cow::Borrowed(token)),
            Self::TokenEnv(token_env) => Ok(Cow::Owned(env::var(token_env)?)),
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        match &self {
            Self::Token(_) => Ok(()),
            Self::TokenEnv(token_env) => match env::var(token_env) {
                Ok(_) => Ok(()),
                Err(err) => bail!("{err} ({token_env})"),
            },
        }
    }
}

#[derive(Deserialize)]
struct Response<R = IgnoredAny> {
    ok: bool,
    #[allow(dead_code)]
    description: Option<String>,
    result: Option<R>,
}

#[derive(Deserialize)]
struct ResultMessage {
    message_id: i64,
}

fn telegram_api(token: impl AsRef<str>, method: impl AsRef<str>) -> String {
    format!(
        "https://api.telegram.org/bot{}/{}",
        token.as_ref(),
        method.as_ref()
    )
}

fn telegram_chat_json(chat: &ConfigChat) -> json::Value {
    match chat {
        ConfigChat::Id(id) => json::Value::Number((*id).into()),
        ConfigChat::Username(username) => json::Value::String(format!("@{username}")),
    }
}

enum Entity<'a> {
    Link(&'a str),
    Quote,
}

pub struct Notifier {
    params: ConfigParams,
    last_live_message: Mutex<Option<SentMessage>>,
}

impl NotifierTrait for Notifier {
    fn notify<'a>(
        &'a self,
        notification: &'a Notification<'_>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(self.notify_impl(notification))
    }
}

impl Notifier {
    pub fn new(params: ConfigParams) -> Self {
        Self {
            params,
            last_live_message: Mutex::new(None),
        }
    }

    fn token(&self) -> anyhow::Result<Cow<str>> {
        self.params
            .token
            .as_ref()
            .unwrap_or_else(|| &Config::platform_global().telegram.as_ref().unwrap().token)
            .get()
            .map_err(|err| anyhow!("failed to read token for telegram: {err}"))
    }

    async fn notify_impl(&self, notification: &Notification<'_>) -> anyhow::Result<()> {
        info!("notifying to '{}'", self.params);

        match &notification.kind {
            NotificationKind::LiveOnline(live_status) => {
                self.notify_live(live_status, notification.source).await
            }
            NotificationKind::LiveTitle(live_status, old_title) => {
                self.notify_live_title(live_status, old_title, notification.source)
                    .await
            }
            NotificationKind::Posts(posts) => self.notify_posts(posts, notification.source).await,
        }
    }

    async fn notify_live(
        &self,
        live_status: &LiveStatus,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        if !self.params.notifications.live_online {
            info!("live_online notification is disabled, skip notifying");
            return Ok(());
        }

        if live_status.online {
            self.notify_live_online(live_status, source).await
        } else {
            self.notify_live_offline(live_status, source).await
        }
    }

    fn make_live_caption(
        &self,
        live_status: &LiveStatus,
        source: &StatusSource,
    ) -> (String, Vec<json::Value>) {
        let caption = format!(
            "[{}] {} {}",
            source.platform_name,
            if live_status.online { "🟢" } else { "🟠" },
            live_status.title
        );
        let caption_entities = vec![json!({
            "type": "text_link",
            "offset": 0,
            "length": caption.encode_utf16().count(),
            "url": live_status.live_url
        })];
        (caption, caption_entities)
    }

    async fn notify_live_online(
        &self,
        live_status: &LiveStatus,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        let token = self.token()?;

        let (caption, caption_entities) = self.make_live_caption(live_status, source);

        let body = json!(
            {
                "chat_id": telegram_chat_json(&self.params.chat),
                "message_thread_id": self.params.thread_id,
                "photo": live_status.cover_image_url,
                "caption": caption,
                "caption_entities": caption_entities
            }
        );
        let resp = reqwest::Client::new()
            .post(telegram_api(token, "sendPhoto"))
            .json(&body)
            .send()
            .await
            .map_err(|err| anyhow!("failed to send request for Telegram: {err}"))?;

        let status = resp.status();
        if !status.is_success() {
            bail!(
                "response from Telegram status is not success. resp: {}, body: {}",
                resp.text().await.unwrap_or_else(|_| "*no text*".into()),
                body
            );
        }

        let text = resp
            .text()
            .await
            .map_err(|err| anyhow!("failed to obtain text from response of Telegram: {err}"))?;
        let resp: Response<ResultMessage> = json::from_str(&text)
            .map_err(|err| anyhow!("failed to deserialize response from Telegram: {err}"))?;
        if !resp.ok {
            bail!("response from Telegram contains error, response '{text}'");
        }

        *self.last_live_message.lock().await = Some(SentMessage {
            // The doc guarantees `result` to be present if `ok` == `true`
            id: resp.result.unwrap().message_id,
        });

        Ok(())
    }

    async fn notify_live_offline(
        &self,
        live_status: &LiveStatus,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        if let Some(last_live_message) = self.last_live_message.lock().await.take() {
            let token = self.token()?;

            let (caption, caption_entities) = self.make_live_caption(live_status, source);

            let body = json!(
                {
                    "chat_id": telegram_chat_json(&self.params.chat),
                    "message_id": last_live_message.id,
                    "caption": caption,
                    "caption_entities": caption_entities
                }
            );
            let resp = reqwest::Client::new()
                .post(telegram_api(token, "editMessageCaption"))
                .json(&body)
                .send()
                .await
                .map_err(|err| anyhow!("failed to send request for Telegram: {err}"))?;

            let status = resp.status();
            if !status.is_success() {
                bail!(
                    "response from Telegram status is not success. resp: {}, body: {}",
                    resp.text().await.unwrap_or_else(|_| "*no text*".into()),
                    body
                );
            }

            let text = resp
                .text()
                .await
                .map_err(|err| anyhow!("failed to obtain text from response of Telegram: {err}"))?;
            let resp: Response<ResultMessage> = json::from_str(&text)
                .map_err(|err| anyhow!("failed to deserialize response from Telegram: {err}"))?;
            if !resp.ok {
                bail!("response from Telegram contains error, response '{text}'");
            }
        }
        Ok(())
    }

    async fn notify_live_title(
        &self,
        live_status: &LiveStatus,
        old_title: &str,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        if !self.params.notifications.live_title {
            info!("live_title notification is disabled, skip notifying");
            return Ok(());
        }
        let token = self.token()?;

        let text = format!(
            "[{}] ✏️ {} ⬅️ {old_title}",
            source.platform_name, live_status.title
        );
        let body = json!(
            {
                "chat_id": telegram_chat_json(&self.params.chat),
                "message_thread_id": self.params.thread_id,
                "text": text,
                "entities": vec![json!({
                    "type": "text_link",
                    "offset": 0,
                    "length": text.encode_utf16().count(),
                    "url": live_status.live_url
                })],
                "link_preview_options": { "is_disabled": true },
                // "disable_notification": true, // TODO: Make it configurable
            }
        );
        let resp = reqwest::Client::new()
            .post(telegram_api(token, "sendMessage"))
            .json(&body)
            .send()
            .await
            .map_err(|err| anyhow!("failed to send request for Telegram: {err}"))?;

        let status = resp.status();
        if !status.is_success() {
            bail!(
                "response from Telegram status is not success. resp: {}, body: {}",
                resp.text().await.unwrap_or_else(|_| "*no text*".into()),
                body
            );
        }

        let text = resp
            .text()
            .await
            .map_err(|err| anyhow!("failed to obtain text from response of Telegram: {err}"))?;
        let resp: Response<ResultMessage> = json::from_str(&text)
            .map_err(|err| anyhow!("failed to deserialize response from Telegram: {err}"))?;
        if !resp.ok {
            bail!("response from Telegram contains error, response '{text}'");
        }

        Ok(())
    }

    async fn notify_posts(
        &self,
        posts: &PostsRef<'_>,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        if !self.params.notifications.post {
            info!("post notification is disabled, skip notifying");
            return Ok(());
        }

        let token = self.token()?;

        let mut errors = vec![];

        for post in &posts.0 {
            if let Err(err) = self.notify_post(token.as_ref(), post, source).await {
                error!("failed to notify post to Telegram: {err}");
                errors.push(err);
            }
        }

        errors
            .into_iter()
            .fold(Ok(()), |res, err| bail!("{res:?} {err}"))
    }

    async fn notify_post(
        &self,
        token: &str,
        post: &Post,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        let mut content = String::new();

        let mut entities = Vec::<(Range<usize>, Entity)>::new();

        write!(content, "[{}] ", source.platform_name)?;

        match &post.repost_from {
            Some(RepostFrom::Recursion(repost_from)) => {
                if !post.content.is_empty() {
                    writeln!(content, "💬 {}", post.content)?;
                }

                let quote_begin = content.encode_utf16().count();
                content.write_str("🔁 ")?;

                let nickname_begin = content.encode_utf16().count();
                content.write_str(&repost_from.user.nickname)?;
                entities.push((
                    nickname_begin..content.encode_utf16().count(),
                    Entity::Link(
                        // In order for Telegram to display more relevant information about the
                        // post, we don't use `profile_url` here
                        //
                        // &repost_from.user.profile_url,
                        &repost_from.url,
                    ),
                ));

                write!(content, ": {}", repost_from.content)?;
                entities.push((quote_begin..content.encode_utf16().count(), Entity::Quote));
            }
            Some(RepostFrom::Legacy {
                is_repost,
                is_quote,
            }) => {
                if *is_repost {
                    content.write_str("🔁 ")?
                } else if *is_quote {
                    content.write_str("🔁💬 ")?
                }
                content.write_str(&post.content)?
            }
            None => content.write_str(&post.content)?,
        }

        let mut body = json!(
            {
                "chat_id": telegram_chat_json(&self.params.chat),
                "message_thread_id": self.params.thread_id,
                "disable_notification": true, // TODO: Make it configurable
            }
        );

        fn entities_to_entities(entities: Vec<(Range<usize>, Entity)>) -> json::Value {
            json::Value::Array(
                entities
                    .into_iter()
                    .map(|(range, entity)| match entity {
                        Entity::Link(url) => json!({
                            "type": "text_link",
                            "offset": range.start,
                            "length": range.end - range.start,
                            "url": url,
                        }),
                        Entity::Quote => json!({
                            "type": "blockquote",
                            "offset": range.start,
                            "length": range.end - range.start,
                        }),
                    })
                    .collect(),
            )
        }

        let attachments = post.attachments_recursive();
        let num_attachments = attachments.len();

        let method = match num_attachments {
            0 | 1 => {
                let body = body.as_object_mut().unwrap();

                // Button "View Post"
                body.insert(
                    "reply_markup".into(),
                    json!({
                        "inline_keyboard": [[{
                            "text": "View Post",
                            "url": post.url,
                        }]]
                    }),
                );

                if num_attachments == 0 {
                    body.insert("text".into(), json!(content));
                    body.insert("entities".into(), entities_to_entities(entities));
                    "sendMessage"
                } else {
                    let attachment = attachments.first().unwrap();

                    match attachment {
                        PostAttachment::Image(image) => {
                            body.insert("photo".into(), json!(image.media_url));
                            body.insert("caption".into(), json!(content));
                            body.insert("caption_entities".into(), entities_to_entities(entities));
                            // TODO: `sendAnimation` for single GIF?
                            "sendPhoto"
                        }
                        PostAttachment::Video(video) => {
                            body.insert("video".into(), json!(video.media_url));
                            body.insert("caption".into(), json!(content));
                            body.insert("caption_entities".into(), entities_to_entities(entities));
                            "sendVideo"
                        }
                    }
                }
            }
            _ => {
                let body = body.as_object_mut().unwrap();

                content.write_str("\n\n")?;
                let button_text_begin = content.encode_utf16().count();
                content.write_str(">> View Post <<")?;
                entities.push((
                    button_text_begin..content.encode_utf16().count(),
                    Entity::Link(&post.url),
                ));

                let mut media = attachments
                    .iter()
                    .map(|attachment| match attachment {
                        PostAttachment::Image(image) => {
                            // TODO: Mixing GIF in media group to send is not yet supported in
                            // Telegram, add an overlay like video? (see
                            // comment in twitter.com implementation)
                            json!({
                                "type": "photo",
                                "media": image.media_url,
                            })
                        }
                        PostAttachment::Video(video) => {
                            json!({
                                "type": "video",
                                "media": video.media_url,
                            })
                        }
                    })
                    .collect::<Vec<_>>();

                let first_media = media.first_mut().unwrap().as_object_mut().unwrap();
                first_media.insert("caption".into(), json!(content));
                first_media.insert("caption_entities".into(), entities_to_entities(entities));

                body.insert("media".into(), json!(media));

                "sendMediaGroup"
            }
        };

        let resp = reqwest::Client::new()
            .post(telegram_api(token, method))
            .json(&body)
            .send()
            .await
            .map_err(|err| anyhow!("failed to send request for Telegram: {err}"))?;

        let status = resp.status();
        if !status.is_success() {
            bail!(
                "response from Telegram status is not success. resp: {}, body: {}",
                resp.text().await.unwrap_or_else(|_| "*no text*".into()),
                body
            );
        }

        let text = resp
            .text()
            .await
            .map_err(|err| anyhow!("failed to obtain text from response of Telegram: {err}"))?;
        let resp: Response = json::from_str(&text)
            .map_err(|err| anyhow!("failed to deserialize response from Telegram: {err}"))?;
        if !resp.ok {
            bail!("response from Telegram contains error, response '{text}'");
        }

        Ok(())
    }
}

struct SentMessage {
    id: i64,
}
