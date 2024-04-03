mod config;
mod notify;
pub mod prop;
mod source;
mod task;

use std::path::Path;

use anyhow::anyhow;
use task::Task;

use crate::config::Config;

pub async fn run(config: impl AsRef<Path>) -> anyhow::Result<()> {
    let config = Config::init(
        tokio::fs::read_to_string(config)
            .await
            .map_err(|err| anyhow!("failed to read config file: {err}"))?,
    )?;

    let tasks = config.subscriptions().map(|(name, subscription)| {
        Task::new(
            name,
            subscription.interval.unwrap_or(config.interval),
            subscription.notify,
            subscription.platform,
        )
    });

    task::run_tasks(tasks).await?.join_all().await;

    Ok(())
}
