use std::{fmt::Display, future::Future, pin::Pin, time::Duration};

use spdlog::prelude::*;
use tokio::{sync::mpsc, time::MissedTickBehavior};

use super::Task;
use crate::{
    config, notify,
    source::{self, sourcer, FetcherTrait, Sourcer, Status, Update},
};

pub struct TaskSubscription {
    name: String,
    interval: Duration,
    notifiers: Vec<Box<dyn notify::NotifierTrait>>,
    sourcer: Option<Sourcer>, // took when the task is running
}

impl TaskSubscription {
    pub fn new(
        name: String,
        interval: Duration,
        notify: Vec<config::Accessor<notify::platform::Config>>,
        source_platform: &config::Accessor<source::platform::Config>,
    ) -> Self {
        Self {
            name,
            interval,
            notifiers: notify.into_iter().map(notify::notifier).collect(),
            sourcer: Some(sourcer(source_platform)),
        }
    }

    // Handler for poll-based subscription
    async fn continuous_fetch(&mut self, fetcher: Box<dyn FetcherTrait>) {
        let mut interval = tokio::time::interval(self.interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let mut last_status = Status::empty();

        loop {
            interval.tick().await;

            let Ok(mut status) = fetcher.fetch_status().await.inspect_err(|err| {
                error!(
                    "failed to fetch status for '{}' on '{}': {err}",
                    self.name, fetcher
                )
            }) else {
                continue;
            };

            status.sort();

            trace!(
                "status of '{}' on '{fetcher}' now is '{status:?}'",
                self.name
            );

            let notifications = status.generate_notifications(&last_status);
            self.notify(notifications, &fetcher).await;

            last_status.update_incrementally(status);
            trace!("subscription '{}' updated once", self.name);
        }
    }

    // Handler for listen-based subscription
    async fn continuous_wait(
        &mut self,
        mut receiver: mpsc::Receiver<Update>,
        platform: impl Display,
    ) {
        while let Some(update) = receiver.recv().await {
            trace!(
                "event of '{}' on '{platform}' received an update '{update:?}'",
                self.name
            );

            let notifications = update.generate_notifications().await;
            self.notify(notifications, &platform).await;
        }
    }

    async fn notify(&self, notifications: Vec<source::Notification<'_>>, platform: &impl Display) {
        for notification in notifications {
            info!(
                "'{}' needs to send a notification for '{platform}': '{notification}'",
                self.name
            );

            for notifier in &self.notifiers {
                notify::notify(&**notifier, &notification).await;
            }
        }
    }
}

impl Task for TaskSubscription {
    fn run(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        match self.sourcer.take().unwrap() {
            Sourcer::Fetcher(fetcher) => Box::pin(self.continuous_fetch(fetcher)),
            Sourcer::Listener(mut listener) => {
                let (sender, receiver) = mpsc::channel(1);
                // TODO: A bit hacky, improve it?
                let platform = listener.to_string();
                Box::pin(async move {
                    tokio::join!(
                        listener.listen(sender),
                        self.continuous_wait(receiver, platform)
                    );
                })
            }
        }
    }
}
