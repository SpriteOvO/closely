pub mod qq;
pub mod telegram;

use std::fmt;

use anyhow::anyhow;
use serde::Deserialize;

use crate::config::{self, Overridable};

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "platform")]
pub enum Config {
    #[serde(rename = "QQ")]
    Qq(config::Accessor<qq::ConfigParams>),
    Telegram(config::Accessor<telegram::ConfigParams>),
}

impl config::Validator for Config {
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Qq(p) => p.validate(),
            Self::Telegram(p) => p.validate(),
        }
        .map_err(|err| anyhow!("[{self}] {err}"))
    }
}

impl Config {
    pub fn override_into(self, new: toml::Value) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        match self {
            Self::Qq(n) => {
                let new: <qq::ConfigParams as config::Overridable>::Override = new.try_into()?;
                Ok(Self::Qq(config::Accessor::new_then_validate(
                    n.into_inner().override_into(new),
                )?))
            }
            Self::Telegram(n) => {
                let new: <telegram::ConfigParams as config::Overridable>::Override =
                    new.try_into()?;
                Ok(Self::Telegram(config::Accessor::new_then_validate(
                    n.into_inner().override_into(new),
                )?))
            }
        }
    }
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Qq(p) => write!(f, "{p}"),
            Self::Telegram(p) => write!(f, "{p}"),
        }
    }
}
