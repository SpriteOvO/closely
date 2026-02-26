pub mod source;

use std::borrow::Cow;

use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::Deserialize;

use crate::{
    config::{Accessor, ConfigCookies, Validator},
    helper, prop,
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigGlobal {
    #[serde(flatten)]
    pub cookies: Accessor<Option<ConfigCookies>>,
    pub space: Accessor<Option<source::space::ConfigGlobal>>,
    pub playback: Accessor<Option<source::playback::ConfigGlobal>>,
}

impl Validator for ConfigGlobal {
    fn validate(&self) -> anyhow::Result<()> {
        self.cookies.validate()?;
        self.space.validate()?;
        self.playback.validate()?;
        Ok(())
    }
}

#[derive(Deserialize)]
struct Response<T> {
    pub(crate) code: i32,
    #[allow(dead_code)]
    pub(crate) message: String,
    pub(crate) data: Option<T>,
}

// TODO: Return Cow
fn upgrade_to_https(url: &str) -> String {
    if url.starts_with("http://") {
        url.replacen("http://", "https://", 1)
    } else {
        url.into()
    }
}

fn normalize_bilibili_url(url: &str) -> Cow<'_, str> {
    if url.starts_with("//") {
        Cow::Owned(format!("https:{url}"))
    } else {
        Cow::Borrowed(url)
    }
}

fn bilibili_request_builder() -> anyhow::Result<reqwest::Client> {
    helper::reqwest_client_with(|builder| {
        builder.default_headers(HeaderMap::from_iter([(
            header::USER_AGENT,
            HeaderValue::from_str(&prop::UserAgent::LogoDynamic.as_str()).unwrap(),
        )]))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrade_https() {
        assert_eq!(
            upgrade_to_https("http://example.com/http://example.com"),
            "https://example.com/http://example.com"
        );
    }
}
