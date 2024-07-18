pub mod cli;
mod config;
mod helper;
mod notify;
mod platform;
pub mod prop;
mod reporter;
mod source;
mod task;

use anyhow::anyhow;
use once_cell::sync::OnceCell;
use task::{Task, TaskReporter, TaskSubscription};

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

    let subscription_tasks = config.subscriptions().map(|(name, subscription)| {
        Box::new(TaskSubscription::new(
            name,
            subscription.interval.unwrap_or(config.interval),
            subscription.notify,
            subscription.platform,
        )) as Box<dyn Task>
    });
    let reporter_task = config
        .reporter()
        .map(|params| Box::new(TaskReporter::new(params)) as Box<dyn Task>);
    let tasks = reporter_task.into_iter().chain(subscription_tasks);

    task::run_tasks(tasks).await?.join_all().await;

    Ok(())
}
