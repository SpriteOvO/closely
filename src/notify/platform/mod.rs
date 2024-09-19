#[cfg(feature = "qq")]
pub mod qq;
pub mod telegram;

use std::fmt;

use anyhow::anyhow;
use serde::Deserialize;

use crate::config::{self, Overridable};

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "platform")]
pub enum Config {
    #[cfg(feature = "qq")]
    #[serde(rename = "QQ")]
    Qq(qq::ConfigParams),
    Telegram(telegram::ConfigParams),
}

impl Config {
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

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "qq")]
            Self::Qq(p) => write!(f, "{p}"),
            Self::Telegram(p) => write!(f, "{p}"),
        }
    }
}
