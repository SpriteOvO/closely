mod telegram;

use std::{future::Future, pin::Pin};

use spdlog::prelude::*;

use crate::{config, source::Notification};

pub trait NotifierTrait: Send + Sync {
    fn notify<'a>(
        &'a self,
        notification: &'a Notification<'_>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>>;
}

pub fn notifier(params: config::Notify) -> Box<dyn NotifierTrait> {
    match params {
        config::Notify::Telegram(p) => Box::new(telegram::Notifier::new(p)),
    }
}

pub async fn notify(notify: &dyn NotifierTrait, notification: &Notification<'_>) {
    info!("notifying notification '{notification}'");
    if let Err(err) = notify.notify(notification).await {
        error!("failed to notify: {err}");
    }
}
