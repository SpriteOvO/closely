use std::{collections::HashMap, fmt, future::Future, pin::Pin};

use anyhow::{anyhow, bail};
use serde::Deserialize;
use serde_json::{self as json, json};
use spdlog::critical;
use tokio::sync::Mutex;

use super::{upgrade_to_https, Response};
use crate::{
    helper,
    platform::{PlatformMetadata, PlatformTrait},
    source::{
        FetcherTrait, LiveStatus, LiveStatusKind, Status, StatusKind, StatusSource,
        StatusSourceUser,
    },
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

enum RoomData {
    Normal(ResponseDataRoom),
    Banned,
}

#[derive(Clone, Debug, Deserialize)]
struct ResponseDataRoom {
    title: String,
    room_id: u64,
    #[allow(dead_code)]
    uid: u64,
    live_status: u64, // 0: offline, 1: online, 2: replay
    uname: String,
    cover_from_user: String, // Empty for no cover (not yet updated)
}

pub struct Fetcher {
    params: ConfigParams,
    room_data_cache: Mutex<Option<ResponseDataRoom>>,
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
        Self {
            params,
            room_data_cache: Mutex::new(None),
        }
    }

    async fn fetch_status_impl(&self) -> anyhow::Result<Status> {
        let data = fetch_live_info(self.params.user_id).await?;

        let mut cache = self.room_data_cache.lock().await;
        let status = match data {
            RoomData::Normal(data) => {
                *cache = Some(data.clone());
                self.room_data_into_status(data, false)
            }
            RoomData::Banned => {
                if let Some(data) = &*cache {
                    self.room_data_into_status(data.clone(), true)
                } else {
                    Status::empty()
                }
            }
        };
        Ok(status)
    }

    fn room_data_into_status(&self, data: ResponseDataRoom, is_banned: bool) -> Status {
        Status::new(
            StatusKind::Live(LiveStatus {
                kind: match (is_banned, data.live_status) {
                    (true, _) => LiveStatusKind::Banned,
                    (false, 0 | 2) => LiveStatusKind::Offline,
                    (false, 1) => LiveStatusKind::Online,
                    (false, _) => {
                        critical!("unexpected live status. data: {data:?}, is_banned: {is_banned}");
                        LiveStatusKind::Offline
                    }
                },
                title: data.title,
                streamer_name: data.uname.clone(),
                cover_image_url: if !data.cover_from_user.is_empty() {
                    upgrade_to_https(&data.cover_from_user)
                } else {
                    "https://i1.hdslb.com/bfs/static/blive/live-assets/common/images/no-cover.png"
                        .into()
                },
                live_url: format!("https://live.bilibili.com/{}", data.room_id),
            }),
            StatusSource {
                platform: self.metadata(),
                user: Some(StatusSourceUser {
                    display_name: data.uname,
                    profile_url: format!("https://space.bilibili.com/{}", self.params.user_id),
                }),
            },
        )
    }
}

async fn fetch_live_info(user_id: u64) -> anyhow::Result<RoomData> {
    let body = json!({ "uids": [user_id] });

    let resp = helper::reqwest_client()?
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
    let json: json::Value = json::from_str(&text)
        .map_err(|err| anyhow!("failed to deserialize response to json value: {err}"))?;

    // If the room is banned, the `data` field will be an empty array instead of an
    // object
    let is_banned = json
        .get("data")
        .and_then(|data| data.as_array())
        .is_some_and(|data| data.is_empty());
    if is_banned {
        return Ok(RoomData::Banned);
    }

    let resp: Response<HashMap<String, ResponseDataRoom>> =
        json::from_str(&text).map_err(|err| anyhow!("failed to deserialize response: {err}"))?;
    if resp.code != 0 {
        bail!("response contains error, response '{text}'");
    }

    resp.data
        .unwrap()
        .into_values()
        .next()
        .ok_or_else(|| {
            anyhow!("UNEXPECTED! response with unexpected data array, response '{text}'")
        })
        .map(RoomData::Normal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deser() {
        fetch_live_info(9617619).await.unwrap();
    }
}
