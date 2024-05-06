#[cfg(feature = "qq")]
pub mod qq;
pub mod telegram;

use std::{fmt, future::Future, pin::Pin};

use anyhow::anyhow;
use serde::Deserialize;
use spdlog::prelude::*;

use crate::{
    config::{self, Overridable},
    platform::PlatformTrait,
    source::Notification,
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "platform")]
pub enum ConfigNotify {
    #[cfg(feature = "qq")]
    #[serde(rename = "QQ")]
    Qq(qq::ConfigParams),
    Telegram(telegram::ConfigParams),
}

impl ConfigNotify {
    pub fn validate(&self, global: &config::PlatformGlobal) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "qq")]
            Self::Qq(p) => p.validate(global),
            Self::Telegram(p) => p.validate(global),
        }
        .map_err(|err| anyhow!("[{self}] {err}"))
    }

    pub fn override_into(self, new: toml::Value) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        match self {
            #[cfg(feature = "qq")]
            Self::Qq(n) => {
                let new: <qq::ConfigParams as config::Overridable>::Override = new.try_into()?;
                Ok(Self::Qq(n.override_into(new)))
            }
            Self::Telegram(n) => {
                let new: <telegram::ConfigParams as config::Overridable>::Override =
                    new.try_into()?;
                Ok(Self::Telegram(n.override_into(new)))
            }
        }
    }
}

impl fmt::Display for ConfigNotify {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "qq")]
            Self::Qq(p) => write!(f, "{p}"),
            Self::Telegram(p) => write!(f, "{p}"),
        }
    }
}

pub trait NotifierTrait: PlatformTrait {
    fn notify<'a>(
        &'a self,
        notification: &'a Notification<'_>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>>;
}

pub fn notifier(params: ConfigNotify) -> Box<dyn NotifierTrait> {
    match params {
        #[cfg(feature = "qq")]
        ConfigNotify::Qq(p) => Box::new(qq::Notifier::new(p)),
        ConfigNotify::Telegram(p) => Box::new(telegram::Notifier::new(p)),
    }
}

pub async fn notify(notify: &dyn NotifierTrait, notification: &Notification<'_>) {
    info!("notifying notification '{notification}'");
    if let Err(err) = notify.notify(notification).await {
        error!("failed to notify: {err}");
    }
}
