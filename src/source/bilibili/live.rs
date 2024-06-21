use std::{collections::HashMap, fmt, future::Future, pin::Pin};

use anyhow::{anyhow, bail};
use serde::Deserialize;
use serde_json::{self as json, json};

use super::Response;
use crate::{
    platform::{PlatformMetadata, PlatformTrait},
    source::{FetcherTrait, LiveStatus, Status, StatusKind, StatusSource, StatusSourceUser},
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigParams {
    pub user_id: u64,
}

impl fmt::Display for ConfigParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bilibili.live:{}", self.user_id)
    }
}

const BILIBILI_LIVE_API: &str =
    "https://api.live.bilibili.com/room/v1/Room/get_status_info_by_uids";

#[derive(Deserialize)]
struct ResponseDataRoom {
    title: String,
    room_id: u64,
    #[allow(dead_code)]
    uid: u64,
    live_status: u64,
    uname: String,
    cover_from_user: String, // Empty for no cover (not yet updated)
}

pub struct Fetcher {
    params: ConfigParams,
}

impl PlatformTrait for Fetcher {
    fn metadata(&self) -> PlatformMetadata {
        PlatformMetadata {
            display_name: "bilibili 直播",
        }
    }
}

impl FetcherTrait for Fetcher {
    fn fetch_status(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<Status>> + Send + '_>> {
        Box::pin(self.fetch_status_impl())
    }
}

impl fmt::Display for Fetcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.params)
    }
}

impl Fetcher {
    pub fn new(params: ConfigParams) -> Self {
        Self { params }
    }

    async fn fetch_status_impl(&self) -> anyhow::Result<Status> {
        let data = fetch_live_info(self.params.user_id).await?;

        Ok(Status {
            kind: StatusKind::Live(LiveStatus {
                online: data.live_status == 1,
                title: data.title,
                streamer_name: data.uname.clone(),
                cover_image_url: if !data.cover_from_user.is_empty() {
                    data.cover_from_user
                } else {
                    "https://i1.hdslb.com/bfs/static/blive/live-assets/common/images/no-cover.png"
                        .into()
                },
                live_url: format!("https://live.bilibili.com/{}", data.room_id),
            }),
            source: StatusSource {
                platform: self.metadata(),
                user: Some(StatusSourceUser {
                    display_name: data.uname,
                    profile_url: format!("https://space.bilibili.com/{}", self.params.user_id),
                }),
            },
        })
    }
}

async fn fetch_live_info(user_id: u64) -> anyhow::Result<ResponseDataRoom> {
    let body = json!({ "uids": [user_id] });

    let resp = reqwest::Client::new()
        .post(BILIBILI_LIVE_API)
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
    let resp: Response<HashMap<String, ResponseDataRoom>> =
        json::from_str(&text).map_err(|err| anyhow!("failed to deserialize response: {err}"))?;
    if resp.code != 0 {
        bail!("response contains error, response '{text}'");
    }

    resp.data.unwrap().into_values().next().ok_or_else(|| {
        anyhow!("UNEXPECTED! response with unexpected data array, response '{text}'")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deser() {
        fetch_live_info(9617619).await.unwrap();
    }
}
