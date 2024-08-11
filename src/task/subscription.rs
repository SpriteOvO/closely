use std::{pin::Pin, time::Duration};

use anyhow::anyhow;
use spdlog::prelude::*;
use tokio::time::MissedTickBehavior;

use super::{Task, TaskKind};
use crate::{
    notify,
    source::{self, ConfigSourcePlatform, Status},
};

pub struct TaskSubscription {
    name: String,
    interval: Duration,
    notifiers: Vec<Box<dyn notify::NotifierTrait>>,
    fetcher: Box<dyn source::FetcherTrait>,
    last_status: Status,
}

impl TaskSubscription {
    pub fn new(
        name: String,
        interval: Duration,
        notify: Vec<notify::ConfigNotify>,
        source_platform: &ConfigSourcePlatform,
    ) -> Self {
        Self {
            name,
            interval,
            notifiers: notify.into_iter().map(notify::notifier).collect(),
            fetcher: source::fetcher(source_platform),
            last_status: Status::empty(),
        }
    }

    async fn run_impl(&mut self) {
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

        let notifications = status.generate_notifications(&self.last_status);
        for notification in notifications {
            if let Some(notification) = self.fetcher.post_filter(notification).await {
                info!(
                    "'{}' needs to send a notification for '{}': '{notification}'",
                    self.name, self.fetcher
                );

                for notifier in &self.notifiers {
                    notify::notify(&**notifier, &notification).await;
                }
            }
        }
        self.last_status.update_incrementally(status);
        Ok(())
    }
}

impl Task for TaskSubscription {
    fn kind(&self) -> TaskKind {
        TaskKind::Poll
    }

    fn run(&mut self) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(self.run_impl())
    }
}
