use std::{borrow::Cow, fmt::Write, future::Future, ops::Range, pin::Pin};

use anyhow::{anyhow, bail};
use serde::{de::IgnoredAny, Deserialize};
use serde_json::{self as json, json};
use spdlog::prelude::*;
use tokio::sync::Mutex;

use super::Notifier;
use crate::{
    config,
    platform::{
        LiveStatus, Notification, NotificationKind, Post, PostAttachment, PostsRef, RepostFrom,
        StatusSource,
    },
};

#[derive(Deserialize)]
struct TelegramResponse<R = IgnoredAny> {
    ok: bool,
    #[allow(dead_code)]
    description: Option<String>,
    result: Option<R>,
}

#[derive(Deserialize)]
struct TelegramResultMessage {
    message_id: i64,
}

fn telegram_api(token: impl AsRef<str>, method: impl AsRef<str>) -> String {
    format!(
        "https://api.telegram.org/bot{}/{}",
        token.as_ref(),
        method.as_ref()
    )
}

pub struct TelegramNotifier {
    params: config::NotifyTelegram,
    last_live_message: Mutex<Option<SentMessage>>,
}

impl Notifier for TelegramNotifier {
    fn notify<'a>(
        &'a self,
        notification: &'a Notification<'_>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(self.notify_impl(notification))
    }
}

impl TelegramNotifier {
    pub fn new(params: config::NotifyTelegram) -> Self {
        Self {
            params,
            last_live_message: Mutex::new(None),
        }
    }

    fn token(&self) -> anyhow::Result<Cow<'_, str>> {
        self.params
            .token()
            .map_err(|err| anyhow!("failed to read token for telegram: {err}"))
    }

    async fn notify_impl(&self, notification: &Notification<'_>) -> anyhow::Result<()> {
        info!("notifying to '{}'", self.params);

        match &notification.kind {
            NotificationKind::Live(live_status) => {
                self.notify_live(live_status, notification.source).await
            }
            NotificationKind::Posts(posts) => self.notify_posts(posts, notification.source).await,
        }
    }

    async fn notify_live(
        &self,
        live_status: &LiveStatus,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
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
                "chat_id": match &self.params.chat {
                    config::NotifyTelegramChat::Id(id) => json::Value::Number((*id).into()),
                    config::NotifyTelegramChat::Username(username) => json::Value::String(format!("@{username}")),
                },
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
            bail!("response from Telegram status is not success: {resp:?}");
        }

        let text = resp
            .text()
            .await
            .map_err(|err| anyhow!("failed to obtain text from response of Telegram: {err}"))?;
        let resp: TelegramResponse<TelegramResultMessage> = json::from_str(&text)
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
                    "chat_id": match &self.params.chat {
                        config::NotifyTelegramChat::Id(id) => json::Value::Number((*id).into()),
                        config::NotifyTelegramChat::Username(username) => json::Value::String(format!("@{username}")),
                    },
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
                bail!("response from Telegram status is not success: {resp:?}");
            }

            let text = resp
                .text()
                .await
                .map_err(|err| anyhow!("failed to obtain text from response of Telegram: {err}"))?;
            let resp: TelegramResponse<TelegramResultMessage> = json::from_str(&text)
                .map_err(|err| anyhow!("failed to deserialize response from Telegram: {err}"))?;
            if !resp.ok {
                bail!("response from Telegram contains error, response '{text}'");
            }
        }
        Ok(())
    }

    async fn notify_posts(
        &self,
        posts: &PostsRef<'_>,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
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

        let mut links = Vec::<(Range<usize>, &str)>::new();

        write!(content, "[{}] ", source.platform_name)?;

        match &post.repost_from {
            Some(RepostFrom::Recursion(repost_from)) => {
                if !post.content.is_empty() {
                    write!(content, "💬 {}\n\n", post.content)?;
                }

                content.write_str("🔁 ")?;

                let nickname_begin = content.encode_utf16().count();
                content.write_str(&repost_from.user.nickname)?;
                links.push((
                    nickname_begin..content.encode_utf16().count(),
                    // In order for Telegram to display more relevant information about the post,
                    // we don't use `profile_url` here
                    //
                    // &repost_from.user.profile_url,
                    &repost_from.url,
                ));

                write!(content, ": {}", repost_from.content)?;
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
                "chat_id": match &self.params.chat {
                    config::NotifyTelegramChat::Id(id) => json::Value::Number((*id).into()),
                    config::NotifyTelegramChat::Username(username) => json::Value::String(format!("@{username}")),
                },
                "message_thread_id": self.params.thread_id,
                "disable_notification": true, // TODO: Make it configurable
            }
        );

        fn links_to_entities(links: Vec<(Range<usize>, &str)>) -> json::Value {
            json::Value::Array(
                links
                    .into_iter()
                    .map(|(range, url)| {
                        json!({
                            "type": "text_link",
                            "offset": range.start,
                            "length": range.end - range.start,
                            "url": url,
                        })
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
                    body.insert("entities".into(), links_to_entities(links));
                    "sendMessage"
                } else {
                    let attachment = attachments.first().unwrap();

                    match attachment {
                        PostAttachment::Image(image) => {
                            body.insert("photo".into(), json!(image.media_url));
                            body.insert("caption".into(), json!(content));
                            body.insert("caption_entities".into(), links_to_entities(links));
                            // TODO: `sendAnimation` for single GIF?
                            "sendPhoto"
                        }
                        PostAttachment::Video(video) => {
                            body.insert("video".into(), json!(video.media_url));
                            body.insert("caption".into(), json!(content));
                            body.insert("caption_entities".into(), links_to_entities(links));
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
                links.push((button_text_begin..content.encode_utf16().count(), &post.url));

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
                first_media.insert("caption_entities".into(), links_to_entities(links));

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
            bail!("response from Telegram status is not success: {resp:?}");
        }

        let text = resp
            .text()
            .await
            .map_err(|err| anyhow!("failed to obtain text from response of Telegram: {err}"))?;
        let resp: TelegramResponse = json::from_str(&text)
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
