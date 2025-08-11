pub mod source;

use reqwest::header::{self, HeaderValue};
use serde::Deserialize;

use crate::{
    config::{Accessor, Validator},
    helper, prop,
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigGlobal {
    pub playback: Accessor<Option<source::playback::ConfigGlobal>>,
}

impl Validator for ConfigGlobal {
    fn validate(&self) -> anyhow::Result<()> {
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

fn upgrade_to_https(url: &str) -> String {
    if url.starts_with("http://") {
        url.replacen("http://", "https://", 1)
    } else {
        url.into()
    }
}

fn bilibili_request_builder() -> anyhow::Result<reqwest::Client> {
    helper::reqwest_client_with(|builder, headers| {
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_str(&prop::UserAgent::LogoDynamic.as_str()).unwrap(),
        );
        builder
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
