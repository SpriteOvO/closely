mod config;
mod notify;
mod platform;
pub mod prop;
mod task;
mod util;

use std::path::Path;

use anyhow::anyhow;
use task::Task;

use crate::config::Config;

pub async fn run(config: impl AsRef<Path>) -> anyhow::Result<()> {
    let config = Config::from_str(
        tokio::fs::read_to_string(config)
            .await
            .map_err(|err| anyhow!("failed to read config file: {err}"))?,
    )?;

    init_from_config(&config)
        .await
        .map_err(|err| anyhow!("failed to init from config: {err}"))?;

    let tasks = config
        .subscriptions()
        .map(|(name, (notify, platform))| Task::new(name, config.interval, notify, platform));

    task::run_tasks(tasks).await?.join_all().await;

    Ok(())
}

async fn init_from_config(config: &Config) -> anyhow::Result<()> {
    // TODO: Check if no twitter global config when contaning twitter subscription
    if let Some(twitter) = &config.platform.twitter {
        platform::twitter_com::init_from_config(twitter).await?;
    }
    Ok(())
}
