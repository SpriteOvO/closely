pub mod bililive_recorder;

use std::{fmt, future::Future, pin::Pin};

use anyhow::anyhow;
use bililive_recorder::*;
use once_cell::sync::Lazy;
use serde::Deserialize;
use spdlog::prelude::*;
use tokio::sync::mpsc;

use crate::{
    config::{Accessor, Config, Validator},
    platform::{PlatformMetadata, PlatformTrait},
    source::{ListenerTrait, Update},
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigGlobal {
    pub bililive_recorder: Accessor<ConfigBililiveRecorder>,
}

impl Validator for ConfigGlobal {
    fn validate(&self) -> anyhow::Result<()> {
        self.bililive_recorder.validate()?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigParams {
    pub room_id: u64,
}

impl Validator for ConfigParams {
    fn validate(&self) -> anyhow::Result<()> {
        Config::global()
            .platform()
            .bilibili
            .as_ref()
            .and_then(|b| b.playback.as_ref())
            .ok_or_else(|| anyhow!("bilibili.playback in global is missing"))?;
        Ok(())
    }
}

impl fmt::Display for ConfigParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bilibili.playback:{}", self.room_id)
    }
}

const PLATFORM_METADATA: PlatformMetadata = PlatformMetadata {
    display_name: "bilibili 录播",
};

static BACKEND: Lazy<BililiveRecorder> = Lazy::new(|| {
    BililiveRecorder::new(
        Config::global()
            .platform()
            .bilibili
            .as_ref()
            .unwrap()
            .playback
            .as_ref()
            .unwrap()
            .bililive_recorder
            .clone(),
    )
});

pub struct Listener {
    params: Accessor<ConfigParams>,
}

impl PlatformTrait for Listener {
    fn metadata(&self) -> PlatformMetadata {
        PLATFORM_METADATA
    }
}

impl ListenerTrait for Listener {
    fn listen(
        &mut self,
        sender: mpsc::Sender<Update>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self.listen_impl(sender))
    }
}

impl fmt::Display for Listener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.params)
    }
}

impl Listener {
    pub fn new(params: Accessor<ConfigParams>) -> Self {
        Self { params }
    }

    async fn listen_impl(&mut self, sender: mpsc::Sender<Update>) {
        BACKEND.add_listener(self.params.room_id, sender).await;

        if let Err(err) = BACKEND.listen().await {
            error!("bilibili.playback failed to listen: {err}");
        }
    }
}
