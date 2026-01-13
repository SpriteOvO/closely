use std::{fmt, future::Future, pin::Pin};

use anyhow::{anyhow, ensure, Ok};
use serde::Deserialize;

use crate::{
    config::{Accessor, Validator},
    platform::{PlatformMetadata, PlatformTraitStatic},
    source::{Article, Articles, FetcherTrait, Status, StatusKind, StatusSource, User},
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigParams {
    pub repo: String,
    pub query: String,
}

impl Validator for ConfigParams {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.repo.split('/').count() == 2 && !self.repo.contains(' '),
            "repo must be in the format 'owner/repo'"
        );
        Ok(())
    }
}

impl fmt::Display for ConfigParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GitHub.issue_pr:{}", self.query)
    }
}

//

pub struct Fetcher {
    params: Accessor<ConfigParams>,
}

impl PlatformTraitStatic for Fetcher {
    fn metadata() -> PlatformMetadata {
        PlatformMetadata {
            display_name: "GitHub",
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
        let issues_prs = fetch(&self.params.repo, &self.params.query).await?;

        Ok(Status::new(
            StatusKind::Articles(issues_prs),
            StatusSource {
                platform: Self::metadata(),
                user: None,
            },
        ))
    }
}

async fn fetch(repo: &str, query: &str) -> anyhow::Result<Articles> {
    let results = octocrab::instance()
        .search()
        .issues_and_pull_requests(&format!("repo:{repo} {query}"))
        .sort("created")
        .per_page(30)
        .send()
        .await
        .map_err(|err| anyhow!("failed to search issue&pr: {err}"))?;

    let results = results
        .items
        .into_iter()
        .map(|item| Article {
            unique_id: item.html_url.to_string(),
            title: item.title,
            body: item.body.unwrap_or_default(),
            link: item.html_url.to_string(),
            author: User {
                nickname: item.user.login,
                profile_url: item.user.html_url.to_string(),
                avatar_url: Some(item.user.avatar_url.to_string()),
            },
            tags: item
                .labels
                .into_iter()
                .map(|label| label.name)
                .collect::<Vec<_>>(),
            created_time: item.created_at.into(),
            kind: Some(if item.pull_request.is_some() {
                "PR"
            } else {
                "Issue"
            }),
        })
        .collect::<Vec<_>>();

    Ok(Articles(results))
}
