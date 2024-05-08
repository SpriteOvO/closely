mod reporter;
mod subscription;

use std::{future::Future, pin::Pin};

pub use reporter::TaskReporter;
use spdlog::prelude::*;
pub use subscription::TaskSubscription;

pub enum TaskKind {
    Noop,
    Poll,
}

pub trait Task: Send {
    fn kind(&self) -> TaskKind;
    fn run(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

pub struct Runner {
    join_handles: Vec<tokio::task::JoinHandle<()>>,
}

impl Runner {
    pub async fn join_all(self) {
        for join_handle in self.join_handles {
            if let Err(err) = join_handle.await {
                error!("failed to join task: {err}");
            }
        }
    }
}

pub async fn run_tasks(tasks: impl IntoIterator<Item = Box<dyn Task>>) -> anyhow::Result<Runner> {
    let join_handles = tasks
        .into_iter()
        .filter_map(|mut task| match task.kind() {
            TaskKind::Noop => None,
            TaskKind::Poll => Some(tokio::spawn(async move {
                task.run().await;
            })),
        })
        .collect::<Vec<_>>();

    info!("{} tasks are running", join_handles.len());

    Ok(Runner { join_handles })
}
