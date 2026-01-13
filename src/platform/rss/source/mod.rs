use std::{fmt, future::Future, pin::Pin};

use anyhow::{anyhow, ensure};
use serde::Deserialize;

use crate::{
    config::{Accessor, Validator},
    platform::{PlatformMetadata, PlatformTraitStatic},
    source::{Feed, Feeds, FetcherTrait, Status, StatusKind, StatusSource},
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigParams {
    pub url: String,
}

impl Validator for ConfigParams {
    fn validate(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl fmt::Display for ConfigParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RSS:{}", self.url)
    }
}

//

pub struct Fetcher {
    params: Accessor<ConfigParams>,
}

impl PlatformTraitStatic for Fetcher {
    fn metadata() -> PlatformMetadata {
        PlatformMetadata {
            display_name: "RSS",
        }
    }
}

impl FetcherTrait for Fetcher {
    fn fetch_status(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<Status>> + Send + '_>> {
        Box::pin(self.fetch_status_impl())
    }
}

impl fmt::Display for Fetcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.params)
    }
}

impl Fetcher {
    pub fn new(params: Accessor<ConfigParams>) -> Self {
        Self { params }
    }

    async fn fetch_status_impl(&self) -> anyhow::Result<Status> {
        let feeds = fetch_feeds(&self.params.url).await?;

        Ok(Status::new(
            StatusKind::Feeds(feeds),
            StatusSource {
                platform: Self::metadata(),
                user: None,
            },
        ))
    }
}

async fn fetch_feeds(url: &str) -> anyhow::Result<Feeds> {
    fn html_to_text(html: String) -> String {
        html2text::config::plain()
            .string_from_read(html.as_bytes(), usize::MAX)
            .unwrap_or(html)
    }

    let resp = reqwest::get(url)
        .await
        .map_err(|err| anyhow!("failed to send request: {err}"))?;
    let status = resp.status();
    ensure!(
        status.is_success(),
        "response status is not success: {resp:?}"
    );

    let content = resp
        .bytes()
        .await
        .map_err(|err| anyhow!("failed to obtain bytes from response: {err}"))?;

    let parsed = feed_rs::parser::parse(&content[..])
        .map_err(|err| anyhow!("failed to parse Feeds: {err}"))?;

    Ok(Feeds {
        title: parsed.title.map(|t| t.content),
        items: parsed
            .entries
            .into_iter()
            .map(|entry| Feed {
                unique_id: entry.id,
                title: entry.title.map(|t| t.content),
                description: entry.summary.map(|s| html_to_text(s.content)),
                link: entry.links.into_iter().next().map(|l| l.href),
                pub_date: entry.published.map(|t| t.into()),
                author: {
                    let authors = entry
                        .authors
                        .into_iter()
                        .map(|a| a.name)
                        .collect::<Vec<_>>();
                    if authors.is_empty() {
                        None
                    } else {
                        Some(authors.join(", "))
                    }
                },
            })
            .collect(),
    })
}
