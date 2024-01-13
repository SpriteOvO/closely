use std::{sync::Arc, time::Duration};

use anyhow::anyhow;
use spdlog::prelude::*;
use tokio::time::MissedTickBehavior;

use crate::{
    config::{Notify, Platform},
    notify,
    platform::{self, LiveStatus},
};

pub struct Task {
    name: String,
    interval: Duration,
    notify: Arc<Notify>,
    platform: Arc<Platform>,
    offline_notification: bool,
    last_status: Option<LiveStatus>,
}

impl Task {
    pub fn new(
        name: String,
        interval: Duration,
        notify: Arc<Notify>,
        platform: Arc<Platform>,
        offline_notification: bool,
    ) -> Self {
        Self {
            name,
            interval,
            notify,
            platform,
            last_status: None,
            offline_notification,
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
        let live_status = platform::fetch_live_status(&self.platform)
            .await
            .map_err(|err| anyhow!("failed to fetch live status: {err}"))?;

        trace!("live status of '{}' now is '{live_status:?}'", self.name);

        if let Some(last_status) = &self.last_status {
            if last_status.online != live_status.online {
                info!("live status of '{}' changed to '{live_status}'", self.name);
                if live_status.online || self.offline_notification {
                    notify::notify(&self.notify, &live_status).await;
                }
            }
        }
        self.last_status = Some(live_status);

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
