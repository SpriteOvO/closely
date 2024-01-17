mod config;
mod notify;
mod platform;
pub mod prop;
mod task;

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

    let tasks = config
        .subscriptions()
        .map(|(name, (notify, platform))| Task::new(name, config.interval, notify, platform));

    task::run_tasks(tasks).await?.join_all().await;

    Ok(())
}
