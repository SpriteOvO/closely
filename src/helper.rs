use std::{convert::identity, time::Duration};

use anyhow::anyhow;

pub fn reqwest_client() -> anyhow::Result<reqwest::Client> {
    reqwest_client_with(identity)
}

pub fn reqwest_client_with(
    configure: impl FnOnce(reqwest::ClientBuilder) -> reqwest::ClientBuilder,
) -> anyhow::Result<reqwest::Client> {
    configure(reqwest::ClientBuilder::new().timeout(Duration::from_secs(30)))
        .build()
        .map_err(|err| anyhow!("failed to build reqwest client: {err}"))
}
