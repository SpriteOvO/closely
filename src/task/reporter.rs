use std::{future::Future, pin::Pin};

use anyhow::ensure;
use spdlog::prelude::*;
use tokio::time::MissedTickBehavior;

use super::Task;
use crate::{
    helper,
    reporter::{ConfigHeartbeat, ConfigHeartbeatKind, ReporterParams},
};

pub struct TaskReporter {
    params: ReporterParams,
}

impl TaskReporter {
    pub fn new(params: ReporterParams) -> Self {
        Self { params }
    }

    async fn run_impl(&self) {
        let heartbeat = self.params.heartbeat.as_ref().unwrap();

        let mut interval = tokio::time::interval(heartbeat.interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            interval.tick().await;
            if let Err(err) = Self::run_once(heartbeat).await {
                error!("error occurred while sending heartbeat: {err}");
            } else {
                trace!("heartbeat sent once");
            }
        }
    }

    async fn run_once(heartbeat: &ConfigHeartbeat) -> anyhow::Result<()> {
        match &heartbeat.kind {
            ConfigHeartbeatKind::HttpGet(http_get) => {
                let response = helper::reqwest_client()?.get(http_get.url()).send().await?;
                let status = response.status();
                ensure!(
                    status.is_success(),
                    "heartbeat server responds unsuccessful status '{status}'. response: {response:?}"
                );
                Ok(())
            }
        }
    }
}

impl Task for TaskReporter {
    fn run(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        match *self.params.heartbeat {
            Some(_) => Box::pin(self.run_impl()),
            None => Box::pin(async {}),
        }
    }
}

// TODO: Mock a server to test it
