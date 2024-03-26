mod telegram;

use std::{future::Future, pin::Pin};

use spdlog::prelude::*;

use self::telegram::TelegramNotifier;
use crate::{config, platform::Notification};

pub trait Notifier: Send + Sync {
    fn notify<'a>(
        &'a self,
        notification: &'a Notification<'_>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>>;
}

pub fn notifier(params: config::Notify) -> Box<dyn Notifier> {
    match params {
        config::Notify::Telegram(p) => Box::new(TelegramNotifier::new(p)),
    }
}

pub async fn notify(notify: &dyn Notifier, notification: &Notification<'_>) {
    info!("notifying notification '{notification}'");
    if let Err(err) = notify.notify(notification).await {
        error!("failed to notify: {err}");
    }
}
