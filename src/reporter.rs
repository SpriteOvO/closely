use std::time::Duration;

use reqwest::Url;
use serde::Deserialize;

use crate::{config, notify};

#[derive(Debug, PartialEq, Deserialize)]
pub struct ConfigReporterRaw {
    #[serde(rename = "notify")]
    pub(crate) notify_ref: Vec<config::NotifyRef>,
    pub(crate) heartbeat: Option<ConfigHeartbeat>,
}

impl ConfigReporterRaw {
    pub fn validate(&self, notify_map: &config::NotifyMap) -> anyhow::Result<()> {
        self.notify_ref
            .iter()
            .map(|notify_ref| notify_map.get_by_ref(notify_ref))
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(heartbeat) = &self.heartbeat {
            heartbeat.validate()?;
        }
        Ok(())
    }

    pub fn reporter(&self, notify_map: &config::NotifyMap) -> ReporterParams {
        ReporterParams {
            notify: self
                .notify_ref
                .iter()
                .map(|notify_ref| notify_map.get_by_ref(notify_ref).unwrap())
                .collect(),
            heartbeat: self.heartbeat.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigHeartbeat {
    #[serde(flatten)]
    pub kind: ConfigHeartbeatKind,
    #[serde(with = "humantime_serde")]
    pub interval: Duration,
}

impl ConfigHeartbeat {
    pub fn validate(&self) -> anyhow::Result<()> {
        self.kind.validate()?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum ConfigHeartbeatKind {
    HttpGet(ConfigHeartbeatHttpGet),
}

impl ConfigHeartbeatKind {
    pub fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::HttpGet(http_get) => http_get.validate(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigHeartbeatHttpGet {
    pub(crate) url: String,
}

impl ConfigHeartbeatHttpGet {
    pub fn validate(&self) -> anyhow::Result<()> {
        Url::parse(&self.url)?;
        Ok(())
    }

    pub fn url(&self) -> Url {
        Url::parse(&self.url).unwrap()
    }
}

pub struct ReporterParams {
    pub notify: Vec<notify::ConfigNotify>, // TODO
    pub heartbeat: Option<ConfigHeartbeat>,
}
