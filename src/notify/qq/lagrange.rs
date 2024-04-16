use std::{
    fmt::Debug,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

use anyhow::{anyhow, ensure};
use rand::distributions::{Alphanumeric, DistString};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{self as json, json};
use spdlog::prelude::*;
use tokio::{fs, time::timeout};

use super::{ConfigChat, ConfigLogin};
use crate::{cli_args, config::AsSecretRef};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigLagrange {
    pub binary_path: PathBuf,
    pub http_port: u16,
    pub sign_server: String,
}

pub struct LograngeOnebot {
    child: Child,
    http_port: u16,
    access_token: String,
}

impl LograngeOnebot {
    pub async fn launch(login: &ConfigLogin, lagrange: &ConfigLagrange) -> anyhow::Result<Self> {
        let access_token = Alphanumeric.sample_string(&mut rand::thread_rng(), 32);

        let appsettings = json!(
            {
                "Logging": {
                    "LogLevel": {
                        "Default": "Information",
                        "Microsoft": "Warning",
                        "Microsoft.Hosting.Lifetime": "Information"
                    }
                },
                "SignServerUrl": lagrange.sign_server,
                "Account": {
                    "Uin": login.account.as_secret_ref().get_parse_copy()?,
                    "Password": login.password.as_secret_ref().get_str()?,
                    "Protocol": "Linux",
                    "AutoReconnect": true,
                    "GetOptimumServer": true
                },
                "Message": {
                    "IgnoreSelf": true,
                    "StringPost": false
                },
                "QrCode": {
                    "ConsoleCompatibilityMode": false
                },
                "Implementations": [
                    {
                        "Type": "Http",
                        "Host": "localhost",
                        "Port": lagrange.http_port,
                        "AccessToken": access_token
                    }
                ]
            }
        );

        #[cfg(debug_assertions)]
        info!("logrange access token: {access_token}");

        let working_dir = lagrange
            .binary_path
            .parent()
            .ok_or_else(|| anyhow!("binary path of logrange has no parent"))?;
        fs::write(
            working_dir.join("appsettings.json"),
            json::to_string_pretty(&appsettings)?,
        )
        .await?;

        let mut command = Command::new(&lagrange.binary_path);

        if let Some(log_dir) = &cli_args().log_dir {
            let log_dir = PathBuf::from(log_dir).join("lagrange");
            std::fs::create_dir_all(&log_dir)
                .map_err(|err| anyhow!("failed to create directories for lagrange logs: {err}"))?;

            let mut open_options = std::fs::OpenOptions::new();
            open_options.create(true).append(true);

            let stdfile = |stream| {
                open_options
                    .open(log_dir.join(format!("lagrange.{stream}")))
                    .map_err(|err| anyhow!("failed to open {stream} log file for lagrange: {err}"))
            };
            command
                .stdout(stdfile("stdout")?)
                .stderr(stdfile("stderr")?);
        } else {
            command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        };

        let child = command.current_dir(working_dir).spawn()?;

        Ok(Self {
            child,
            http_port: lagrange.http_port,
            access_token,
        })
    }

    async fn request<T: DeserializeOwned + Debug>(
        &self,
        method: &str,
        arguments: Option<json::Value>,
    ) -> anyhow::Result<Response<T>> {
        async {
            let resp = reqwest::Client::new()
                .post(format!("http://localhost:{}/{method}", self.http_port))
                .json(&arguments.unwrap_or(json::Value::Null))
                .bearer_auth(&self.access_token)
                .send()
                .await?;
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

impl Drop for LograngeOnebot {
    fn drop(&mut self) {
        _ = self.child.kill();
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
    pub message_id: u64,
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

    pub fn build(self) -> Message {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
enum MessageSegment {
    Text(MessageSegmentText),
    Image(MessageSegmentImage),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct MessageSegmentText {
    text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct MessageSegmentImage {
    file: String,
}
