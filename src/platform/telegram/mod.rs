pub mod notify;

use std::fmt;

use http::Uri;
use serde::Deserialize;
use serde_json as json;
use spdlog::prelude::*;

use crate::{config::Validator, secret_enum, serde_impl_default_for};

// Base
//

secret_enum! {
    #[derive(Clone, Debug, PartialEq, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ConfigToken {
        Token(String),
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigChat {
    Id(i64),
    Username(String),
}

impl ConfigChat {
    fn to_json(&self) -> json::Value {
        match self {
            ConfigChat::Id(id) => json::Value::Number((*id).into()),
            ConfigChat::Username(username) => json::Value::String(format!("@{username}")),
        }
    }
}

impl fmt::Display for ConfigChat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigChat::Id(id) => write!(f, "{id}"),
            ConfigChat::Username(username) => write!(f, "@{username}"),
        }
    }
}

// Global
//

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigGlobal {
    #[serde(flatten)]
    pub token: Option<ConfigToken>,
    #[serde(default)]
    pub api_server: Option<ConfigApiServer>,
    #[serde(default)]
    pub experimental: ConfigExperimental,
}

impl Validator for ConfigGlobal {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(token) = &self.token {
            token.validate()?;
        }
        #[allow(deprecated)]
        if self.experimental.send_live_image_as_preview.is_some() {
            warn!("config option 'platform.Telegram.experimental.send_live_image_as_preview' is deprecated, it's now always enabled");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum ConfigApiServer {
    Url(#[serde(with = "http_serde::uri")] Uri),
    UrlOpts {
        #[serde(with = "http_serde::uri")]
        url: Uri,
        as_necessary: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigExperimental {
    #[deprecated = "enabled by default"]
    pub send_live_image_as_preview: Option<bool>,
}

serde_impl_default_for!(ConfigExperimental);
