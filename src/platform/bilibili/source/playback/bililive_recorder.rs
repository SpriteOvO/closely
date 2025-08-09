use std::{
    collections::HashMap, convert::Infallible, mem, net::SocketAddr, path::PathBuf, sync::Arc,
};

use anyhow::{anyhow, bail};
use bytes::Bytes;
use chrono::{DateTime, Local};
use serde::Deserialize;
use serde_json as json;
use spdlog::prelude::*;
use tokio::{
    fs,
    sync::{mpsc, Mutex},
};
use warp::Filter;

use super::PLATFORM_METADATA;
use crate::{
    config::{Accessor, Validator},
    source::{Document, Playback, PlaybackFormat, StatusSource, Update, UpdateKind},
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigBililiveRecorder {
    pub listen_webhook: ConfigListen,
    pub working_directory: PathBuf,
}

impl Validator for ConfigBililiveRecorder {
    fn validate(&self) -> anyhow::Result<()> {
        self.listen_webhook.to_addr()?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigListen {
    pub host: String,
    pub port: u16,
}

impl ConfigListen {
    fn to_addr(&self) -> anyhow::Result<SocketAddr> {
        Ok(SocketAddr::new(
            self.host.parse().map_err(|err| {
                anyhow!("invalid host '{}' for bilibili.playback: {err}", self.host)
            })?,
            self.port,
        ))
    }
}

pub struct BililiveRecorder {
    config: Accessor<ConfigBililiveRecorder>,
    senders: Arc<Mutex<HashMap<u64, mpsc::Sender<Update>>>>,
    is_listening: Mutex<bool>,
}

impl BililiveRecorder {
    pub fn new(config: Accessor<ConfigBililiveRecorder>) -> Self {
        Self {
            config,
            senders: Arc::new(Mutex::new(HashMap::new())),
            is_listening: Mutex::new(false),
        }
    }

    pub async fn add_listener(&self, room_id: u64, sender: mpsc::Sender<Update>) {
        assert!(self.senders.lock().await.insert(room_id, sender).is_none());
    }

    // Only block on the first call
    pub async fn listen(&self) -> anyhow::Result<()> {
        if mem::replace(&mut *self.is_listening.lock().await, true) {
            return Ok(());
        }

        let ctx = Arc::new(Context {
            working_directory: self.config.working_directory.clone(),
            sessions: Mutex::new(HashMap::new()),
            senders: Arc::clone(&self.senders),
        });

        let routes = warp::post()
            // .and(warp::body::json()) // Can't find a way to log all requests
            .and(warp::body::bytes())
            .and(with_context(ctx))
            .and_then(webhook_handler);

        warp::serve(routes)
            .run(self.config.listen_webhook.to_addr().unwrap())
            .await;
        Ok(())
    }
}

fn with_context(
    ctx: Arc<Context>,
) -> impl Filter<Extract = (Arc<Context>,), Error = Infallible> + Clone {
    warp::any().map(move || Arc::clone(&ctx))
}

#[derive(Clone, Debug)]
struct Session {
    live_start_time: Option<DateTime<Local>>,
    room_id: u64,
}

struct Context {
    working_directory: PathBuf,
    sessions: Mutex<HashMap<u64 /* room_id */, Session>>,
    senders: Arc<Mutex<HashMap<u64, mpsc::Sender<Update>>>>,
}

async fn webhook_handler(
    event: Bytes, // data::WebhookV2,
    ctx: Arc<Context>,
) -> Result<warp::http::StatusCode, warp::Rejection> {
    let debug_str = String::from_utf8_lossy(&event);
    trace!("recevied a webhook call from bililive-recorder: {debug_str}");

    if let Ok(event) = json::from_slice(&event).inspect_err(|err| {
        error!("failed to deserialize bililive-recorder webhook request. {err} '{debug_str}'");
    }) {
        if let Err(err) = handle(event, &ctx).await {
            error!("bililive-recorder webhook handler error: {err}");
        }
    }

    // Always respond 200 OK to Bililive Recorder as it will retry on error
    Ok(warp::http::StatusCode::OK)
}

async fn handle(event: data::WebhookV2, params: &Context) -> anyhow::Result<()> {
    match event.event_kind {
        data::EventKind::SessionStarted(session_started) => {
            let session = Session {
                live_start_time: parse_timestamp(&event.timestamp)
                    .inspect_err(|err| {
                        warn!(
                            "bililive-recorder failed to parse timestamp '{}': {err}",
                            event.timestamp
                        )
                    })
                    .ok(),
                room_id: session_started.room_id,
            };
            if params
                .sessions
                .lock()
                .await
                .insert(session_started.room_id, session)
                .is_some()
            {
                warn!("started an existing session '{session_started:?}'");
            }
            Ok(())
        }
        data::EventKind::SessionEnded(session_ended) => {
            if params
                .sessions
                .lock()
                .await
                .remove(&session_ended.room_id)
                .is_none()
            {
                warn!("ended a non-existing session '{session_ended:?}'");
            }
            Ok(())
        }
        data::EventKind::FileClosed(file_closed) => {
            let session = params
                .sessions
                .lock()
                .await
                .get(&file_closed.room_id)
                .cloned()
                .unwrap_or_else(|| {
                    warn!(
                        "bililive-recorder closed a file with an unknown session '{file_closed:?}'"
                    );
                    Session {
                        live_start_time: None,
                        room_id: file_closed.room_id,
                    }
                });

            #[derive(Clone, Copy)]
            enum FileType {
                Video(PlaybackFormat),
                Metadata,
            }

            let send_update = async |path: PathBuf, file_type| {
                let status = Update::new(
                    match file_type {
                        FileType::Video(format) => UpdateKind::Playback(Playback {
                            live_start_time: session.live_start_time,
                            file_path: path,
                            format,
                        }),
                        FileType::Metadata => UpdateKind::Document(Document { file_path: path }),
                    },
                    StatusSource {
                        platform: PLATFORM_METADATA,
                        user: None,
                    },
                );

                params
                    .senders
                    .lock()
                    .await
                    .get(&session.room_id)
                    .ok_or_else(|| anyhow!("room id {} has no sender", session.room_id))?
                    .send(status)
                    .await
                    .map_err(|err| anyhow!("failed to send status: {err}. session: {session:?}"))
            };

            let playback_file = params.working_directory.join(&file_closed.relative_path);
            let playback_type = match playback_file
                .extension()
                .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
                .as_deref()
            {
                Some("flv") => FileType::Video(PlaybackFormat::Flv),
                Some("xml") => FileType::Metadata,
                _ => bail!(
                    "bililive-recorder closed a file with an unknown extension '{playback_file:?}'"
                ),
            };

            let metadate_file = playback_file.with_extension("xml");

            send_update(playback_file, playback_type).await?;
            if let FileType::Video(_) = playback_type {
                if let Ok(true) = fs::try_exists(&metadate_file).await {
                    send_update(metadate_file, FileType::Metadata).await?;
                }
            }

            Ok(())
        }
        data::EventKind::FileOpening {}
        | data::EventKind::StreamStarted {}
        | data::EventKind::StreamEnded {} => Ok(()),
    }
}

fn parse_timestamp(timestamp: &str) -> anyhow::Result<DateTime<Local>> {
    Ok(DateTime::parse_from_str(timestamp, "%FT%T%.f%:z")?.into())
}

mod data {
    use super::*;

    #[derive(Deserialize, PartialEq, Debug)]
    pub struct WebhookV2 {
        #[serde(flatten)]
        pub event_kind: EventKind,
        #[serde(rename = "EventTimestamp")]
        pub timestamp: String,
        #[serde(rename = "EventId")]
        pub id: String,
    }

    #[derive(Deserialize, PartialEq, Debug)]
    #[serde(tag = "EventType", content = "EventData")]
    pub enum EventKind {
        SessionStarted(SessionStarted),
        SessionEnded(SessionEnded),
        FileOpening {},
        FileClosed(FileClosed),
        StreamStarted {},
        StreamEnded {},
    }

    #[derive(Deserialize, PartialEq, Debug)]
    pub struct SessionStarted {
        #[serde(rename = "SessionId")]
        pub session_id: String,
        #[serde(rename = "RoomId")]
        pub room_id: u64,
        #[serde(rename = "ShortId")]
        pub short_id: u64,
        #[serde(rename = "Name")]
        pub name: String,
        #[serde(rename = "Title")]
        pub title: String,
        #[serde(rename = "AreaNameParent")]
        pub area_name_parent: String,
        #[serde(rename = "AreaNameChild")]
        pub area_name_child: String,
        #[serde(rename = "Recording")]
        pub recording: bool,
        #[serde(rename = "Streaming")]
        pub streaming: bool,
        #[serde(rename = "DanmakuConnected")]
        pub danmaku_connected: bool,
    }

    #[derive(Deserialize, PartialEq, Debug)]
    pub struct SessionEnded {
        #[serde(rename = "SessionId")]
        pub session_id: String,
        #[serde(rename = "RoomId")]
        pub room_id: u64,
        #[serde(rename = "ShortId")]
        pub short_id: u64,
        #[serde(rename = "Name")]
        pub name: String,
        #[serde(rename = "Title")]
        pub title: String,
        #[serde(rename = "AreaNameParent")]
        pub area_name_parent: String,
        #[serde(rename = "AreaNameChild")]
        pub area_name_child: String,
        #[serde(rename = "Recording")]
        pub recording: bool,
        #[serde(rename = "Streaming")]
        pub streaming: bool,
        #[serde(rename = "DanmakuConnected")]
        pub danmaku_connected: bool,
    }

    #[derive(Deserialize, PartialEq, Debug)]
    pub struct FileClosed {
        #[serde(rename = "RelativePath")]
        pub relative_path: String,
        #[serde(rename = "FileSize")]
        pub file_size: u64,
        #[serde(rename = "Duration")]
        pub duration: f32,
        #[serde(rename = "FileOpenTime")]
        pub file_open_time: String,
        #[serde(rename = "FileCloseTime")]
        pub file_close_time: String,
        #[serde(rename = "SessionId")]
        pub session_id: String,
        #[serde(rename = "RoomId")]
        pub room_id: u64,
        #[serde(rename = "ShortId")]
        pub short_id: u64,
        #[serde(rename = "Name")]
        pub name: String,
        #[serde(rename = "Title")]
        pub title: String,
        #[serde(rename = "AreaNameParent")]
        pub area_name_parent: String,
        #[serde(rename = "AreaNameChild")]
        pub area_name_child: String,
        #[serde(rename = "Recording")]
        pub recording: bool,
        #[serde(rename = "Streaming")]
        pub streaming: bool,
        #[serde(rename = "DanmakuConnected")]
        pub danmaku_connected: bool,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deser() {
        let session_started = r#"
        {
            "EventType": "SessionStarted",
            "EventTimestamp": "2021-05-14T17:52:44.4960899+08:00",
            "EventId": "e3e1c9ec-f386-4bc3-9e5a-661bf3ed2fb2",
            "EventData": {
                "SessionId": "7c7f3672-70ce-405a-aa12-886702ced6e5",
                "RoomId": 23058,
                "ShortId": 3,
                "Name": "3号直播间",
                "Title": "哔哩哔哩音悦台",
                "AreaNameParent": "生活",
                "AreaNameChild": "影音馆",
                "Recording": true,
                "Streaming": true,
                "DanmakuConnected": true
            }
        }"#;
        assert_eq!(
            json::from_str::<data::WebhookV2>(session_started).unwrap(),
            data::WebhookV2 {
                event_kind: data::EventKind::SessionStarted(data::SessionStarted {
                    session_id: "7c7f3672-70ce-405a-aa12-886702ced6e5".to_string(),
                    room_id: 23058,
                    short_id: 3,
                    name: "3号直播间".to_string(),
                    title: "哔哩哔哩音悦台".to_string(),
                    area_name_parent: "生活".to_string(),
                    area_name_child: "影音馆".to_string(),
                    recording: true,
                    streaming: true,
                    danmaku_connected: true,
                }),
                timestamp: "2021-05-14T17:52:44.4960899+08:00".to_string(),
                id: "e3e1c9ec-f386-4bc3-9e5a-661bf3ed2fb2".to_string(),
            }
        );

        let session_ended = r#"
        {
            "EventType": "SessionEnded",
            "EventTimestamp": "2021-05-14T17:52:54.9481095+08:00",
            "EventId": "e1f4a36e-e34c-4ada-80bb-f6cfc90e99e9",
            "EventData": {
                "SessionId": "7c7f3672-70ce-405a-aa12-886702ced6e5",
                "RoomId": 23058,
                "ShortId": 3,
                "Name": "3号直播间",
                "Title": "哔哩哔哩音悦台",
                "AreaNameParent": "生活",
                "AreaNameChild": "影音馆",
                "Recording": true,
                "Streaming": true,
                "DanmakuConnected": true
            }
        }"#;
        assert_eq!(
            json::from_str::<data::WebhookV2>(session_ended).unwrap(),
            data::WebhookV2 {
                event_kind: data::EventKind::SessionEnded(data::SessionEnded {
                    session_id: "7c7f3672-70ce-405a-aa12-886702ced6e5".to_string(),
                    room_id: 23058,
                    short_id: 3,
                    name: "3号直播间".to_string(),
                    title: "哔哩哔哩音悦台".to_string(),
                    area_name_parent: "生活".to_string(),
                    area_name_child: "影音馆".to_string(),
                    recording: true,
                    streaming: true,
                    danmaku_connected: true,
                }),
                timestamp: "2021-05-14T17:52:54.9481095+08:00".to_string(),
                id: "e1f4a36e-e34c-4ada-80bb-f6cfc90e99e9".to_string(),
            }
        );

        let file_opening = r#"
        {
            "EventType": "FileOpening",
            "EventTimestamp": "2021-05-14T17:52:50.5256394+08:00",
            "EventId": "6e7b33e5-4695-4d25-87ee-b09f66e20ba0",
            "EventData": {
                "RelativePath": "23058-3号直播间/录制-23058-20210514-175250-哔哩哔哩音悦台.flv",
                "FileOpenTime": "2021-05-14T17:52:50.5246401+08:00",
                "SessionId": "7c7f3672-70ce-405a-aa12-886702ced6e5",
                "RoomId": 23058,
                "ShortId": 3,
                "Name": "3号直播间",
                "Title": "哔哩哔哩音悦台",
                "AreaNameParent": "生活",
                "AreaNameChild": "影音馆",
                "Recording": true,
                "Streaming": true,
                "DanmakuConnected": true
            }
        }"#;
        assert!(matches!(
            json::from_str::<data::WebhookV2>(file_opening)
                .unwrap()
                .event_kind,
            data::EventKind::FileOpening {}
        ));

        let file_closed = r#"
        {
            "EventType": "FileClosed",
            "EventTimestamp": "2021-05-14T17:52:54.9461101+08:00",
            "EventId": "98f85267-e08c-4f15-ad9a-1fc463d42b0b",
            "EventData": {
                "RelativePath": "23058-3号直播间/录制-23058-20210514-175250-哔哩哔哩音悦台.flv",
                "FileSize": 816412,
                "Duration": 4.992,
                "FileOpenTime": "2021-05-14T17:52:50.5246401+08:00",
                "FileCloseTime": "2021-05-14T17:52:54.9461101+08:00",
                "SessionId": "7c7f3672-70ce-405a-aa12-886702ced6e5",
                "RoomId": 23058,
                "ShortId": 3,
                "Name": "3号直播间",
                "Title": "哔哩哔哩音悦台",
                "AreaNameParent": "生活",
                "AreaNameChild": "影音馆",
                "Recording": true,
                "Streaming": true,
                "DanmakuConnected": true
            }
        }"#;
        assert_eq!(
            json::from_str::<data::WebhookV2>(file_closed).unwrap(),
            data::WebhookV2 {
                event_kind: data::EventKind::FileClosed(data::FileClosed {
                    relative_path: "23058-3号直播间/录制-23058-20210514-175250-哔哩哔哩音悦台.flv"
                        .to_string(),
                    file_size: 816412,
                    duration: 4.992,
                    file_open_time: "2021-05-14T17:52:50.5246401+08:00".to_string(),
                    file_close_time: "2021-05-14T17:52:54.9461101+08:00".to_string(),
                    session_id: "7c7f3672-70ce-405a-aa12-886702ced6e5".to_string(),
                    room_id: 23058,
                    short_id: 3,
                    name: "3号直播间".to_string(),
                    title: "哔哩哔哩音悦台".to_string(),
                    area_name_parent: "生活".to_string(),
                    area_name_child: "影音馆".to_string(),
                    recording: true,
                    streaming: true,
                    danmaku_connected: true,
                }),
                timestamp: "2021-05-14T17:52:54.9461101+08:00".to_string(),
                id: "98f85267-e08c-4f15-ad9a-1fc463d42b0b".to_string(),
            }
        );
    }

    #[test]
    fn timestamp_format() {
        assert_eq!(
            parse_timestamp("2021-05-14T17:52:44.4960899+08:00")
                .unwrap()
                .to_utc()
                .to_string(),
            "2021-05-14 09:52:44.496089900 UTC"
        );
    }
}
