use std::{convert::identity, time::Duration};

use anyhow::anyhow;
use reqwest::header::{self, HeaderMap, HeaderValue};

use crate::prop;

pub fn reqwest_client() -> anyhow::Result<reqwest::Client> {
    reqwest_client_with(identity)
}

pub fn reqwest_client_with(
    configure: impl FnOnce(reqwest::ClientBuilder) -> reqwest::ClientBuilder,
) -> anyhow::Result<reqwest::Client> {
    configure(
        reqwest::ClientBuilder::new()
            .timeout(Duration::from_secs(30))
            .default_headers(HeaderMap::from_iter([(
                header::USER_AGENT,
                HeaderValue::from_str(&prop::user_agent(false)).unwrap(),
            )])),
    )
    .build()
    .map_err(|err| anyhow!("failed to build reqwest client: {err}"))
}
