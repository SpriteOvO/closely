use anyhow::{anyhow, bail};
use serde::Deserialize;
use serde_json::{self as json, json};

use crate::{
    config::{NotifyTelegram, NotifyTelegramChat},
    platform::LiveStatus,
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

pub async fn notify(notify: &NotifyTelegram, live_status: &LiveStatus) -> anyhow::Result<()> {
    let body = json!(
        {
            "chat_id": match &notify.chat {
                NotifyTelegramChat::Id(id) => json::Value::Number((*id).into()),
                NotifyTelegramChat::Username(username) => json::Value::String(format!("@{username}")),
            },
            "message_thread_id": notify.thread_id,
            "photo": live_status.cover_image_url,
            "caption": live_status.title,
            "caption_entities": [
                {
                    "type": "text_link",
                    "offset": 0,
                    "length": live_status.title.encode_utf16().count(),
                    "url": live_status.live_url
                }
            ]
        }
    );

    let token = notify
        .token()
        .map_err(|err| anyhow!("failed to read token for telegram: {err}"))?;
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
