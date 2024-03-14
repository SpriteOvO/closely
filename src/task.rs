use std::time::Duration;

use anyhow::anyhow;
use spdlog::prelude::*;
use tokio::time::MissedTickBehavior;

use crate::{
    config::{Notify, Platform},
    notify,
    platform::{self, Status},
};

pub struct Task {
    name: String,
    interval: Duration,
    notifiers: Vec<Box<dyn notify::Notifier>>,
    fetcher: Box<dyn platform::Fetcher>,
    last_status: Option<Status>,
}

impl Task {
    pub fn new(
        name: String,
        interval: Duration,
        notify: Vec<&Notify>,
        platform: &Platform,
    ) -> Self {
        Self {
            name,
            interval,
            notifiers: notify.into_iter().map(notify::notifier).collect(),
            fetcher: platform::fetcher(platform),
            last_status: None,
        }
    }

    pub async fn run(mut self) {
        let mut interval = tokio::time::interval(self.interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            interval.tick().await;
            if let Err(err) = self.run_once().await {
                error!(
                    "error occurred while updating subscription '{}': {err}",
                    self.name
                );
            }
            trace!("subscription '{}' updated once", self.name);
        }
    }
}

impl Task {
    async fn run_once(&mut self) -> anyhow::Result<()> {
        let status = self.fetcher.fetch_status().await.map_err(|err| {
            anyhow!(
                "failed to fetch status for '{}' on '{}': {err}",
                self.name,
                self.fetcher
            )
        })?;

        trace!(
            "status of '{}' on '{}' now is '{status:?}'",
            self.name,
            self.fetcher
        );

        if let Some(notification) = self
            .fetcher
            .post_filter_opt(status.needs_notify(self.last_status.as_ref()))
            .await
        {
            info!(
                "'{}' needs to send a notification for '{}': '{notification}'",
                self.name, self.fetcher
            );

            for notifier in &self.notifiers {
                notify::notify(&**notifier, &notification).await;
            }
        }
        self.last_status = Some(status);

        Ok(())
    }
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

pub async fn run_tasks(tasks: impl IntoIterator<Item = Task>) -> anyhow::Result<Runner> {
    let join_handles = tasks
        .into_iter()
        .map(|task| {
            tokio::spawn(async move {
                task.run().await;
            })
        })
        .collect();

    Ok(Runner { join_handles })
}
