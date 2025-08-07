use std::{borrow::Cow, fmt::Debug, time::Duration};

use anyhow::{anyhow, ensure};
use serde::{de::DeserializeOwned, ser::SerializeStruct, Deserialize, Serialize, Serializer};
use serde_json::{self as json, json};
use tokio::time::timeout;

use super::ConfigChat;
use crate::helper;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct RemoteHttp {
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigLagrange {
    pub remote_http: RemoteHttp,
    pub access_token: Option<String>,
}

pub struct LagrangeOnebot<'a> {
    config: &'a ConfigLagrange,
}

impl<'a> LagrangeOnebot<'a> {
    pub fn new(config: &'a ConfigLagrange) -> Self {
        Self { config }
        // instance
        //     .version_info_retry_timeout(Duration::from_secs(5))
        //     .await
        //     .map_err(|_| {
        //         anyhow!(
        //             "failed to connect Lagrange on '{}:{}'",
        //             config.http_host,
        //             config.http_port
        //         )
        //     })?;
    }

    async fn request<T: DeserializeOwned + Debug>(
        &self,
        method: &str,
        arguments: Option<json::Value>,
    ) -> anyhow::Result<Response<T>> {
        async {
            let mut resp = helper::reqwest_client()?
                .post(format!(
                    "http://{}:{}/{method}",
                    self.config.remote_http.host, self.config.remote_http.port
                ))
                .json(&arguments.unwrap_or(json::Value::Null));
            if let Some(access_token) = self.config.access_token.as_ref() {
                resp = resp.bearer_auth(access_token);
            }
            let resp = resp.send().await?;

            let status = resp.status();
            ensure!(
                status.is_success(),
                "response status is not success '{status}'"
            );
            let resp: Response<T> = resp.json().await?;
            ensure!(
                resp.retcode == 0,
                "response contains error, response '{resp:?}'"
            );
            Ok(resp)
        }
        .await
        .map_err(|err: anyhow::Error| {
            anyhow!("failed to request to lagrange. method: '{method}', err: {err}")
        })
    }

    pub async fn version_info_retry_timeout(
        &self,
        duration: Duration,
    ) -> anyhow::Result<VersionInfo> {
        timeout(duration, async {
            loop {
                match self.version_info().await {
                    Ok(version_info) => break Ok(version_info),
                    Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
                }
            }
        })
        .await
        .map_err(|err| anyhow!("timeout while waiting for version info: {err}"))?
    }

    pub async fn version_info(&self) -> anyhow::Result<VersionInfo> {
        self.request("get_version_info", None)
            .await
            .map(|resp| resp.data.unwrap())
    }

    pub async fn send_message(
        &self,
        chat: &ConfigChat,
        message: Message,
    ) -> anyhow::Result<MessageId> {
        let mut args = json!(
            {
                "message_type": match chat {
                    ConfigChat::GroupId(_) => "group",
                    ConfigChat::UserId(_) => "private"
                },
                "message": message,
            }
        );
        match chat {
            ConfigChat::GroupId(id) => args["group_id"] = json!(*id),
            ConfigChat::UserId(id) => args["user_id"] = json!(*id),
        }
        self.request::<_>("send_msg", Some(args))
            .await
            .map(|resp| resp.data.unwrap())
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Response<T> {
    pub status: String,
    pub retcode: u64,
    pub data: Option<T>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct VersionInfo {
    pub app_name: String,
    pub app_version: String,
    pub protocol_version: String,
    pub nt_protocol: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct MessageId {
    pub message_id: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Message(Vec<MessageSegment>);

impl Message {
    pub fn builder() -> MessageBuilder {
        MessageBuilder(Message(vec![]))
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::builder().text(text).build()
    }
}

pub struct MessageBuilder(Message);

impl MessageBuilder {
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.0 .0.push(MessageSegment::Text(MessageSegmentText {
            text: text.into(),
        }));
        self
    }

    pub fn image(mut self, file: impl Into<String>) -> Self {
        self.0 .0.push(MessageSegment::Image(MessageSegmentImage {
            file: file.into(),
        }));
        self
    }

    pub fn images(mut self, files: impl IntoIterator<Item = impl Into<String>>) -> Self {
        files.into_iter().for_each(|file| {
            self.0 .0.push(MessageSegment::Image(MessageSegmentImage {
                file: file.into(),
            }))
        });
        self
    }

    pub fn mention(mut self, user_id: u64, newline: bool) -> Self {
        if newline {
            self = self.text("\n");
        }
        self.0
             .0
            .push(MessageSegment::At(MessageSegmentAt::UserId(user_id)));
        self
    }

    pub fn mention_all(mut self, newline: bool) -> Self {
        if newline {
            self = self.text("\n");
        }
        self.0 .0.push(MessageSegment::At(MessageSegmentAt::All));
        self
    }

    pub fn mention_all_if(self, cond: bool, newline: bool) -> Self {
        if cond {
            return self.mention_all(newline);
        }
        self
    }

    pub fn build(self) -> Message {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
enum MessageSegment {
    Text(MessageSegmentText),
    Image(MessageSegmentImage),
    At(MessageSegmentAt),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct MessageSegmentText {
    text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct MessageSegmentImage {
    file: String,
}

#[derive(Clone, Debug, PartialEq)]
enum MessageSegmentAt {
    UserId(u64),
    All,
}

impl Serialize for MessageSegmentAt {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = match self {
            Self::UserId(id) => Cow::Owned(id.to_string()),
            Self::All => Cow::Borrowed("all"),
        };
        let mut at = serializer.serialize_struct("MessageSegmentAt", 1)?;
        at.serialize_field("qq", &value)?;
        at.end()
    }
}
