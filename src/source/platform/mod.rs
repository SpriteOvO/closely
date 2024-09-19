pub mod bilibili;
pub mod twitter;

use std::fmt;

use serde::Deserialize;

use crate::config::PlatformGlobal;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "name")]
#[allow(clippy::enum_variant_names)]
pub enum Config {
    #[serde(rename = "bilibili.live")]
    BilibiliLive(bilibili::live::ConfigParams),
    #[serde(rename = "bilibili.space")]
    BilibiliSpace(bilibili::space::ConfigParams),
    #[serde(rename = "bilibili.video")]
    BilibiliVideo(bilibili::video::ConfigParams),
    #[serde(rename = "Twitter")]
    Twitter(twitter::ConfigParams),
}

impl Config {
    pub fn validate(&self, global: &PlatformGlobal) -> anyhow::Result<()> {
        match self {
            Self::BilibiliLive(_) | Self::BilibiliSpace(_) | Self::BilibiliVideo(_) => Ok(()),
            Self::Twitter(p) => p.validate(global),
        }
    }
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BilibiliLive(p) => write!(f, "{p}"),
            Self::BilibiliSpace(p) => write!(f, "{p}"),
            Self::BilibiliVideo(p) => write!(f, "{p}"),
            Self::Twitter(p) => write!(f, "{p}"),
        }
    }
}
