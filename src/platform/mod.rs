mod live_bilibili_com;

use std::fmt;

use anyhow::anyhow;
use spdlog::prelude::*;

use crate::config::Platform;

#[derive(Debug)]
pub struct LiveStatus {
    pub online: bool,
    pub title: String,
    pub streamer_name: String,
    pub cover_image_url: String,
    pub live_url: String,
}

impl fmt::Display for LiveStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "'{}' {}",
            self.streamer_name,
            if self.online { "online" } else { "offline" }
        )?;
        if self.online {
            write!(f, " with title {}", self.title)?;
        }
        Ok(())
    }
}

pub async fn fetch_live_status(platform: &Platform) -> anyhow::Result<LiveStatus> {
    trace!("fetch live status '{platform}'");

    match platform {
        Platform::LiveBilibiliCom(p) => live_bilibili_com::fetch_live_status(p).await,
    }
    .map_err(|err| anyhow!("({platform}) {err}"))
}
