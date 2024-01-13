use std::collections::HashMap;

use anyhow::{anyhow, bail};
use serde::Deserialize;
use serde_json::{self as json, json};

use super::LiveStatus;
use crate::config::PlatformBilibili;

const BILIBILI_API: &str = "https://api.live.bilibili.com/room/v1/Room/get_status_info_by_uids";

#[derive(Deserialize)]
struct BilibiliResponse {
    code: i32,
    #[allow(dead_code)]
    msg: String,
    #[allow(dead_code)]
    message: String,
    data: HashMap<String, BilibiliResponseData>,
}

#[derive(Deserialize)]
struct BilibiliResponseData {
    title: String,
    room_id: u64,
    #[allow(dead_code)]
    uid: u64,
    live_status: u64,
    uname: String,
    cover_from_user: String,
}

pub(super) async fn fetch_live_status(platform: &PlatformBilibili) -> anyhow::Result<LiveStatus> {
    let body = json!({ "uids": [platform.uid] });

    let resp = reqwest::Client::new()
        .post(BILIBILI_API)
        .json(&body)
        .send()
        .await
        .map_err(|err| anyhow!("failed to send request for bilibili: {err}"))?;

    let status = resp.status();
    if !status.is_success() {
        bail!("response from bilibili status is not success: {resp:?}");
    }

    let text = resp
        .text()
        .await
        .map_err(|err| anyhow!("failed to obtain text from response of bilibili: {err}"))?;
    let resp: BilibiliResponse = json::from_str(&text)
        .map_err(|err| anyhow!("failed to deserialize response from bilibili: {err}"))?;
    if resp.code != 0 {
        bail!("response from bilibili contains error, response '{text}'");
    }

    let data = resp.data.into_values().next().ok_or_else(|| {
        anyhow!("UNEXPECTED! response from bilibili with unexpected data array, response '{text}'")
    })?;

    Ok(LiveStatus {
        online: data.live_status == 1,
        title: data.title,
        streamer_name: data.uname,
        cover_image_url: data.cover_from_user,
        live_url: format!("https://live.bilibili.com/{}", data.room_id),
    })
}
