mod abstruct;
pub mod diff;
pub mod platform;

use std::{fmt, future::Future, pin::Pin};

pub use abstruct::*;
use tokio::sync::mpsc;

use crate::{config, platform::PlatformTrait};

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

pub fn sourcer(platform: &config::Accessor<platform::Config>) -> Sourcer {
    match &**platform {
        platform::Config::BilibiliLive(p) => {
            Sourcer::new_fetcher(platform::bilibili::live::Fetcher::new(p.clone()))
        }
        platform::Config::BilibiliSpace(p) => {
            Sourcer::new_fetcher(platform::bilibili::space::Fetcher::new(p.clone()))
        }
        platform::Config::BilibiliVideo(p) => {
            Sourcer::new_fetcher(platform::bilibili::video::Fetcher::new(p.clone()))
        }
        platform::Config::BilibiliPlayback(p) => {
            Sourcer::new_listener(platform::bilibili::playback::Listener::new(p.clone()))
        }
        platform::Config::Twitter(p) => {
            Sourcer::new_fetcher(platform::twitter::Fetcher::new(p.clone()))
        }
    }
}
