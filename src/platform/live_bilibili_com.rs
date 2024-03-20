use std::{collections::HashMap, fmt, future::Future, pin::Pin};

use anyhow::{anyhow, bail};
use serde::Deserialize;
use serde_json::{self as json, json};

use super::{
    Fetcher, LiveStatus, PlatformName, Status, StatusKind, StatusSource, StatusSourceUser,
};
use crate::config::PlatformLiveBilibiliCom;

const LIVE_BILIBILI_COM_API: &str =
    "https://api.live.bilibili.com/room/v1/Room/get_status_info_by_uids";

#[derive(Deserialize)]
pub(crate) struct BilibiliResponse<T> {
    pub(crate) code: i32,
    #[allow(dead_code)]
    pub(crate) message: String,
    pub(crate) data: T,
}

#[derive(Deserialize)]
struct BilibiliResponseDataRoom {
    title: String,
    room_id: u64,
    #[allow(dead_code)]
    uid: u64,
    live_status: u64,
    uname: String,
    cover_from_user: String,
}

pub struct LiveBilibiliComFetcher {
    params: PlatformLiveBilibiliCom,
}

impl Fetcher for LiveBilibiliComFetcher {
    fn fetch_status(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<Status>> + Send + '_>> {
        Box::pin(self.fetch_status_impl())
    }
}

impl fmt::Display for LiveBilibiliComFetcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.params)
    }
}

impl LiveBilibiliComFetcher {
    pub fn new(params: PlatformLiveBilibiliCom) -> Self {
        Self { params }
    }

    async fn fetch_status_impl(&self) -> anyhow::Result<Status> {
        let data = fetch_bilibili_live_info(self.params.uid).await?;

        Ok(Status {
            kind: StatusKind::Live(LiveStatus {
                online: data.live_status == 1,
                title: data.title,
                streamer_name: data.uname.clone(),
                cover_image_url: data.cover_from_user,
                live_url: format!("https://live.bilibili.com/{}", data.room_id),
            }),
            source: StatusSource {
                platform_name: PlatformName::LiveBilibiliCom,
                user: Some(StatusSourceUser {
                    display_name: data.uname,
                    profile_url: format!("https://space.bilibili.com/{}", self.params.uid),
                }),
            },
        })
    }
}

async fn fetch_bilibili_live_info(uid: u64) -> anyhow::Result<BilibiliResponseDataRoom> {
    let body = json!({ "uids": [uid] });

    let resp = reqwest::Client::new()
        .post(LIVE_BILIBILI_COM_API)
        .json(&body)
        .send()
        .await
        .map_err(|err| anyhow!("failed to send request: {err}"))?;

    let status = resp.status();
    if !status.is_success() {
        bail!("response status is not success: {resp:?}");
    }

    let text = resp
        .text()
        .await
        .map_err(|err| anyhow!("failed to obtain text from response: {err}"))?;
    let resp: BilibiliResponse<HashMap<String, BilibiliResponseDataRoom>> =
        json::from_str(&text).map_err(|err| anyhow!("failed to deserialize response: {err}"))?;
    if resp.code != 0 {
        bail!("response contains error, response '{text}'");
    }

    resp.data.into_values().next().ok_or_else(|| {
        anyhow!("UNEXPECTED! response with unexpected data array, response '{text}'")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_live_bilibili_deser() {
        fetch_bilibili_live_info(9617619).await.unwrap();
    }
}
