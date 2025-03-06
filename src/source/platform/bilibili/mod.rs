pub mod live;
pub mod space;
pub mod video;

use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::Deserialize;

use crate::{helper, prop};

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
