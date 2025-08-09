use std::{fmt, future::Future, pin::Pin};

use anyhow::anyhow;
use serde::Deserialize;
use spdlog::prelude::*;

use crate::{
    config::{self, Overridable},
    platform::*,
    source::Notification,
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "platform")]
pub enum NotifierConfig {
    #[serde(rename = "QQ")]
    Qq(config::Accessor<qq::notify::ConfigParams>),
    Telegram(config::Accessor<telegram::notify::ConfigParams>),
}

impl config::Validator for NotifierConfig {
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Qq(p) => p.validate(),
            Self::Telegram(p) => p.validate(),
        }
        .map_err(|err| anyhow!("[{self}] {err}"))
    }
}

impl NotifierConfig {
    pub fn override_into(self, new: toml::Value) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        match self {
            Self::Qq(n) => {
                let new: <qq::notify::ConfigParams as config::Overridable>::Override =
                    new.try_into()?;
                Ok(Self::Qq(config::Accessor::new_then_validate(
                    n.into_inner().override_into(new),
                )?))
            }
            Self::Telegram(n) => {
                let new: <telegram::notify::ConfigParams as config::Overridable>::Override =
                    new.try_into()?;
                Ok(Self::Telegram(config::Accessor::new_then_validate(
                    n.into_inner().override_into(new),
                )?))
            }
        }
    }
}

impl fmt::Display for NotifierConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Qq(p) => write!(f, "{p}"),
            Self::Telegram(p) => write!(f, "{p}"),
        }
    }
}

pub trait NotifierTrait: PlatformTrait {
    fn notify<'a>(
        &'a self,
        notification: &'a Notification<'_>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;
}

pub fn notifier(params: config::Accessor<NotifierConfig>) -> Box<dyn NotifierTrait> {
    match params.into_inner() {
        NotifierConfig::Qq(p) => Box::new(qq::notify::Notifier::new(p)),
        NotifierConfig::Telegram(p) => Box::new(telegram::notify::Notifier::new(p)),
    }
}

pub async fn notify(notify: &dyn NotifierTrait, notification: &Notification<'_>) {
    info!("notifying notification '{notification}'");
    if let Err(err) = notify.notify(notification).await {
        error!(
            "failed to notify to {}: {err}",
            notify.metadata().display_name
        );
    }
}
