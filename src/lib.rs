pub mod cli;
mod config;
mod notify;
pub mod prop;
mod source;
mod task;

use anyhow::anyhow;
use once_cell::sync::OnceCell;
use task::Task;

use crate::config::Config;

static CLI_ARGS: OnceCell<cli::Args> = OnceCell::new();

pub fn cli_args() -> &'static cli::Args {
    CLI_ARGS.get().expect("global cli args not initialized")
}

pub async fn run(args: cli::Args) -> anyhow::Result<()> {
    CLI_ARGS
        .set(args)
        .expect("global cli args is already initialized");

    let config = Config::init(
        tokio::fs::read_to_string(&cli_args().config)
            .await
            .map_err(|err| anyhow!("failed to read config file: {err}"))?,
    )
    .await?;

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
