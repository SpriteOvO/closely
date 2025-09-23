pub mod cli;
mod config;
mod helper;
mod notify;
mod platform;
pub mod prop;
mod reporter;
mod source;
mod task;

use std::sync::OnceLock;

use anyhow::anyhow;
use task::{Task, TaskReporter, TaskSubscription};

use crate::config::Config;

static CLI_ARGS: OnceLock<cli::Args> = OnceLock::new();

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

    let initial_offsets = config.equidistant_intervals.then(|| {
        task::equidistant_intervals(
            config
                .subscriptions()
                .map(|(_, subscription)| subscription.interval),
        )
    });

    let subscription_tasks = config
        .subscriptions()
        .enumerate()
        .map(|(i, (name, subscription))| {
            Box::new(TaskSubscription::new(
                name,
                subscription.interval,
                initial_offsets.as_ref().map(|v| *v.get(i).unwrap()),
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
