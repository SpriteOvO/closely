mod equidistant;
mod reporter;
mod subscription;

use std::{future::Future, pin::Pin};

pub use equidistant::equidistant_intervals;
pub use reporter::TaskReporter;
use spdlog::prelude::*;
pub use subscription::TaskSubscription;
use tokio::task::JoinSet;

pub trait Task: Send {
    fn run(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

pub struct Runner {
    join_set: JoinSet<()>,
}

impl Runner {
    pub async fn join_all(mut self) {
        while let Some(join_handle) = self.join_set.join_next().await {
            if let Err(err) = join_handle {
                if err.is_panic() {
                    error!("task panicked", kv: { err: });
                    panic!("task panicked: {err}");
                } else {
                    error!("failed to join task", kv: { err: });
                }
            }
        }
    }
}

pub async fn run_tasks(tasks: impl IntoIterator<Item = Box<dyn Task>>) -> anyhow::Result<Runner> {
    let join_set = tasks
        .into_iter()
        .fold(JoinSet::new(), |mut join_set, mut task| {
            join_set.spawn(async move { task.run().await });
            join_set
        });

    info!("tasks are running", kv: { count = join_set.len() });

    Ok(Runner { join_set })
}
