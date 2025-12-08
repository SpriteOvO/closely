use std::{fmt, future::Future, pin::Pin};

use serde::Deserialize;

use crate::{
    config::{Accessor, AccountRef, AsSecretRef, Config, Validator},
    platform::{
        twitter::{
            request::TwitterCookies,
            source::{validate_actor, FetcherInner},
        },
        PlatformMetadata, PlatformTraitStatic,
    },
    source::{FetcherTrait, Status, StatusKind, StatusSource},
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigParams {
    pub username: String,
    #[serde(rename = "as")]
    pub actor: AccountRef,
}

impl Validator for ConfigParams {
    fn validate(&self) -> anyhow::Result<()> {
        validate_actor(&self.actor)?;
        Ok(())
    }
}

impl fmt::Display for ConfigParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Twitter.post:{}", self.username)
    }
}

//

pub struct Fetcher {
    params: Accessor<ConfigParams>,
    inner: FetcherInner,
}

impl PlatformTraitStatic for Fetcher {
    fn metadata() -> PlatformMetadata {
        PlatformMetadata {
            display_name: "Twitter",
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
        let cookies = Config::global()
            .platform()
            .twitter
            .as_ref()
            .unwrap()
            .account
            .get(&params.actor)
            .as_secret_ref()
            .get_str()
            .unwrap();
        Self {
            params,
            inner: FetcherInner::new(TwitterCookies::new(cookies).unwrap()),
        }
    }

    async fn fetch_status_impl(&self) -> anyhow::Result<Status> {
        let posts = self.inner.user_tweets(&self.params.username).await?;

        Ok(Status::new(
            StatusKind::Posts(posts),
            StatusSource {
                platform: Self::metadata(),
                user: None, // TODO: Implement it later if needed
            },
        ))
    }
}
