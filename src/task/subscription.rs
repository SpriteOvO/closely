use std::{fmt::Display, future::Future, pin::Pin, time::Duration};

use humantime_serde::re::humantime;
use spdlog::prelude::*;
use tokio::{sync::mpsc, time::MissedTickBehavior};

use super::Task;
use crate::{
    config::Accessor,
    notify::{NotifierConfig, NotifierTrait, notifier, notify},
    source::{FetcherTrait, Notification, SourceConfig, Sourcer, Status, Update, sourcer},
};

pub struct TaskSubscription {
    name: String,
    interval: Duration,
    initial_offset: Option<Duration>,
    notifiers: Vec<Box<dyn NotifierTrait>>,
    sourcer: Option<Sourcer>, // took when the task is running
}

impl TaskSubscription {
    pub fn new(
        name: String,
        interval: Duration,
        initial_offset: Option<Duration>,
        notify: Vec<Accessor<NotifierConfig>>,
        source_platform: &Accessor<SourceConfig>,
    ) -> Self {
        trace!(
            "task subscription created",
            kv: {
                subscription = name,
                source: = source_platform,
                interval: = humantime::format_duration(interval),
                initial_offset: = humantime::format_duration(initial_offset.unwrap_or_default())
            }
        );
        Self {
            name,
            interval,
            initial_offset,
            notifiers: notify.into_iter().map(notifier).collect(),
            sourcer: Some(sourcer(source_platform)),
        }
    }

    // Handler for poll-based subscription
    async fn continuous_fetch(&mut self, fetcher: Box<dyn FetcherTrait>) {
        let mut last_status = Status::empty();

        // Fetch for the first time immediately
        self.continuous_fetch_once(&*fetcher, &mut last_status)
            .await;

        if let Some(initial_offset) = self.initial_offset {
            tokio::time::sleep(initial_offset).await;
        }

        let mut interval = tokio::time::interval(self.interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        interval.tick().await; // Skip the first immediate tick since we already fetched once.

        loop {
            interval.tick().await;
            self.continuous_fetch_once(&*fetcher, &mut last_status)
                .await;
        }
    }

    async fn continuous_fetch_once(
        &mut self,
        fetcher: &dyn FetcherTrait,
        last_status: &mut Status,
    ) {
        let Ok(mut status) = fetcher.fetch_status().await.inspect_err(
            |err| error!("failed to fetch status for subscription", kv: { err:, subscription = self.name, fetcher: }),
        ) else {
            return;
        };

        status.sort();

        trace!("subscription status fetched", kv: { subscription = self.name, fetcher:, status:? });

        let notifications = status.generate_notifications(last_status);
        self.notify(notifications, &fetcher).await;

        last_status.update_incrementally(status);
        trace!("subscription updated once", kv: { subscription = self.name });
    }

    // Handler for listen-based subscription
    async fn continuous_wait(
        &mut self,
        mut receiver: mpsc::Receiver<Update>,
        platform: impl Display,
    ) {
        while let Some(update) = receiver.recv().await {
            trace!("subscription received an update '{update:?}'", kv: { subscription = self.name, platform:, update:? });

            let notifications = update.generate_notifications().await;
            self.notify(notifications, &platform).await;
        }
    }

    async fn notify(&self, notifications: Vec<Notification<'_>>, platform: &impl Display) {
        for notification in notifications {
            info!("subscription needs to send a notification", kv: { subscription = self.name, platform:, notification: });

            for notifier in &self.notifiers {
                notify(&**notifier, &notification).await;
            }
        }
    }
}

impl Task for TaskSubscription {
    fn run(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        match self.sourcer.take().unwrap() {
            Sourcer::Fetcher(fetcher) => Box::pin(self.continuous_fetch(fetcher)),
            Sourcer::Listener(mut listener) => {
                let (sender, receiver) = mpsc::channel(10);
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
