pub mod telegram;

use std::{fmt, future::Future, pin::Pin};

use anyhow::{anyhow, ensure};
use serde::Deserialize;
use spdlog::prelude::*;

use crate::{
    config::{self, AsSecretRef, Overridable},
    source::Notification,
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "platform")]
pub enum ConfigNotify {
    Telegram(telegram::ConfigParams),
}

impl ConfigNotify {
    pub fn validate(&self, global: &config::PlatformGlobal) -> anyhow::Result<()> {
        match self {
            ConfigNotify::Telegram(v) => match &v.token {
                Some(token) => token
                    .as_secret_ref()
                    .validate()
                    .map_err(|err| anyhow!("[Telegram] {err}")),
                None => {
                    ensure!(
                        global.telegram.is_some(),
                        "[Telegram] both token in global and notify are missing"
                    );
                    Ok(())
                }
            },
        }
    }

    pub fn override_into(self, new: toml::Value) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        match self {
            ConfigNotify::Telegram(n) => {
                let new: <telegram::ConfigParams as config::Overridable>::Override =
                    new.try_into()?;
                Ok(ConfigNotify::Telegram(n.override_into(new)))
            }
        }
    }
}

impl fmt::Display for ConfigNotify {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigNotify::Telegram(notify_telegram) => write!(f, "{}", notify_telegram),
        }
    }
}

pub trait NotifierTrait: Send + Sync {
    fn notify<'a>(
        &'a self,
        notification: &'a Notification<'_>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>>;
}

pub fn notifier(params: ConfigNotify) -> Box<dyn NotifierTrait> {
    match params {
        ConfigNotify::Telegram(p) => Box::new(telegram::Notifier::new(p)),
    }
}

pub async fn notify(notify: &dyn NotifierTrait, notification: &Notification<'_>) {
    info!("notifying notification '{notification}'");
    if let Err(err) = notify.notify(notification).await {
        error!("failed to notify: {err}");
    }
}
