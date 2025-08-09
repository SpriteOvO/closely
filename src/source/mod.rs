mod abstruct;
pub mod diff;

use std::{fmt, future::Future, pin::Pin};

pub use abstruct::*;
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::{
    config::{Accessor, Validator},
    platform::*,
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "name")]
#[allow(clippy::enum_variant_names)]
pub enum SourceConfig {
    #[serde(rename = "bilibili.live")]
    BilibiliLive(Accessor<bilibili::source::live::ConfigParams>),
    #[serde(rename = "bilibili.space")]
    BilibiliSpace(Accessor<bilibili::source::space::ConfigParams>),
    #[serde(rename = "bilibili.video")]
    BilibiliVideo(Accessor<bilibili::source::video::ConfigParams>),
    #[serde(rename = "bilibili.playback")]
    BilibiliPlayback(Accessor<bilibili::source::playback::ConfigParams>),
    #[serde(rename = "Twitter")]
    Twitter(Accessor<twitter::source::ConfigParams>),
}

impl Validator for SourceConfig {
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

impl fmt::Display for SourceConfig {
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

pub trait FetcherTrait: PlatformTrait + fmt::Display {
    fn fetch_status(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<Status>> + Send + '_>>;
}

pub trait ListenerTrait: PlatformTrait + fmt::Display {
    fn listen(
        &mut self,
        sender: mpsc::Sender<Update>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

pub enum Sourcer {
    // Poll-based
    Fetcher(Box<dyn FetcherTrait>),

    // Listen-based
    Listener(Box<dyn ListenerTrait>),
}

impl Sourcer {
    fn new_fetcher(f: impl FetcherTrait + 'static) -> Self {
        Self::Fetcher(Box::new(f))
    }

    fn new_listener(l: impl ListenerTrait + 'static) -> Self {
        Self::Listener(Box::new(l))
    }
}

pub fn sourcer(platform: &Accessor<SourceConfig>) -> Sourcer {
    match &**platform {
        SourceConfig::BilibiliLive(p) => {
            Sourcer::new_fetcher(bilibili::source::live::Fetcher::new(p.clone()))
        }
        SourceConfig::BilibiliSpace(p) => {
            Sourcer::new_fetcher(bilibili::source::space::Fetcher::new(p.clone()))
        }
        SourceConfig::BilibiliVideo(p) => {
            Sourcer::new_fetcher(bilibili::source::video::Fetcher::new(p.clone()))
        }
        SourceConfig::BilibiliPlayback(p) => {
            Sourcer::new_listener(bilibili::source::playback::Listener::new(p.clone()))
        }
        SourceConfig::Twitter(p) => Sourcer::new_fetcher(twitter::source::Fetcher::new(p.clone())),
    }
}
