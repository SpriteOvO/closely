pub mod bilibili;
pub mod twitter;

use std::fmt;

use serde::Deserialize;

use crate::config;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "name")]
#[allow(clippy::enum_variant_names)]
pub enum Config {
    #[serde(rename = "bilibili.live")]
    BilibiliLive(config::Accessor<bilibili::live::ConfigParams>),
    #[serde(rename = "bilibili.space")]
    BilibiliSpace(config::Accessor<bilibili::space::ConfigParams>),
    #[serde(rename = "bilibili.video")]
    BilibiliVideo(config::Accessor<bilibili::video::ConfigParams>),
    #[serde(rename = "bilibili.playback")]
    BilibiliPlayback(config::Accessor<bilibili::playback::ConfigParams>),
    #[serde(rename = "Twitter")]
    Twitter(config::Accessor<twitter::ConfigParams>),
}

impl config::Validator for Config {
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::BilibiliLive(p) => p.validate(),
            Self::BilibiliSpace(p) => p.validate(),
            Self::BilibiliVideo(p) => p.validate(),
            Self::BilibiliPlayback(p) => p.validate(),
            Self::Twitter(p) => p.validate(),
        }
    }
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BilibiliLive(p) => write!(f, "{p}"),
            Self::BilibiliSpace(p) => write!(f, "{p}"),
            Self::BilibiliVideo(p) => write!(f, "{p}"),
            Self::BilibiliPlayback(p) => write!(f, "{p}"),
            Self::Twitter(p) => write!(f, "{p}"),
        }
    }
}
