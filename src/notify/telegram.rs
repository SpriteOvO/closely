use anyhow::{anyhow, bail};
use serde::Deserialize;
use serde_json::{self as json, json};
use spdlog::prelude::*;

use crate::{
    config::{NotifyTelegram, NotifyTelegramChat},
    platform::{
        LiveStatus, Notification, NotificationKind, Post, PostAttachment, PostsRef, StatusFrom,
    },
};

#[derive(Deserialize)]
struct TelegramResponse {
    ok: bool,
    #[allow(dead_code)]
    description: Option<String>,
    #[allow(dead_code)]
    result: Option<TelegramResponseResult>,
}

#[derive(Deserialize)]
struct TelegramResponseResult {}

fn telegram_api(token: impl AsRef<str>, method: impl AsRef<str>) -> String {
    format!(
        "https://api.telegram.org/bot{}/{}",
        token.as_ref(),
        method.as_ref()
    )
}

pub async fn notify(
    notify: &NotifyTelegram,
    notification: &Notification<'_>,
) -> anyhow::Result<()> {
    let token = notify
        .token()
        .map_err(|err| anyhow!("failed to read token for telegram: {err}"))?;

    match &notification.kind {
        NotificationKind::Live(live_status) => {
            notify_live(notify, token, live_status, notification.from).await
        }
        NotificationKind::Posts(posts) => {
            notify_posts(notify, token, posts, notification.from).await
        }
    }
}

pub async fn notify_live(
    notify: &NotifyTelegram,
    token: impl AsRef<str>,
    live_status: &LiveStatus,
    from: &StatusFrom,
) -> anyhow::Result<()> {
    let title = format!("[{}] {}", from.platform_name, live_status.title);
    let body = json!(
        {
            "chat_id": match &notify.chat {
                NotifyTelegramChat::Id(id) => json::Value::Number((*id).into()),
                NotifyTelegramChat::Username(username) => json::Value::String(format!("@{username}")),
            },
            "message_thread_id": notify.thread_id,
            "photo": live_status.cover_image_url,
            "caption": title,
            "caption_entities": [
                {
                    "type": "text_link",
                    "offset": 0,
                    "length": title.encode_utf16().count(),
                    "url": live_status.live_url
                }
            ]
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
    let resp: TelegramResponse = json::from_str(&text)
        .map_err(|err| anyhow!("failed to deserialize response from Telegram: {err}"))?;
    if !resp.ok {
        bail!("response from Telegram contains error, response '{text}'");
    }

    Ok(())
}

pub async fn notify_posts(
    notify: &NotifyTelegram,
    token: impl AsRef<str>,
    posts: &PostsRef<'_>,
    from: &StatusFrom,
) -> anyhow::Result<()> {
    let mut errors = vec![];

    for post in &posts.0 {
        if let Err(err) = notify_post(notify, token.as_ref(), post, from).await {
            error!("failed to notify post to Telegram: {err}");
            errors.push(err);
        }
    }

    errors
        .into_iter()
        .fold(Ok(()), |res, err| bail!("{res:?} {err}"))
}

pub async fn notify_post(
    notify: &NotifyTelegram,
    token: &str,
    post: &Post,
    from: &StatusFrom,
) -> anyhow::Result<()> {
    let content = format!("[{}] {}", from.platform_name, post.content);

    let mut body = json!(
        {
            "chat_id": match &notify.chat {
                NotifyTelegramChat::Id(id) => json::Value::Number((*id).into()),
                NotifyTelegramChat::Username(username) => json::Value::String(format!("@{username}")),
            },
            "message_thread_id": notify.thread_id,
            "disable_notification": true, // TODO: Make it configurable
        }
    );

    let num_attachments = post.attachments.len();
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
                "sendMessage"
            } else {
                let attachment = post.attachments.first().unwrap();

                match attachment {
                    PostAttachment::Image(image) => {
                        body.insert("photo".into(), json!(image.media_url));
                        body.insert("caption".into(), json!(content));
                        "sendPhoto"
                    }
                    PostAttachment::Video(video) => {
                        body.insert("video".into(), json!(video.media_url));
                        body.insert("caption".into(), json!(content));
                        "sendVideo"
                    }
                }
            }
        }
        _ => {
            let body = body.as_object_mut().unwrap();

            let view_text = ">> View Post <<";
            let (caption, entities) = (
                format!("{}\n\n{view_text}", content),
                json!([
                    {
                        "type": "text_link",
                        "offset": content.encode_utf16().count() + "\n\n".encode_utf16().count(),
                        "length": view_text.encode_utf16().count(),
                        "url": post.url,
                    }
                ]),
            );

            let mut media = post
                .attachments
                .iter()
                .map(|attachment| match attachment {
                    PostAttachment::Image(image) => {
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
            first_media.insert("caption".into(), json!(caption));
            first_media.insert("caption_entities".into(), entities);

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
