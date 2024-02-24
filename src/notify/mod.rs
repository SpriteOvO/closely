mod telegram;

use std::{future::Future, pin::Pin};

use anyhow::bail;
use spdlog::prelude::*;

use self::telegram::TelegramNotifier;
use crate::{config, platform::Notification};

pub trait Notifier: Send + Sync {
    fn notify<'a>(
        &'a self,
        notification: &'a Notification<'_>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>>;
}

struct NotifierVec(Vec<Box<dyn Notifier>>);

impl Notifier for NotifierVec {
    fn notify<'a>(
        &'a self,
        notification: &'a Notification<'_>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(self.notify_impl(notification))
    }
}

impl NotifierVec {
    async fn notify_impl(&self, notification: &Notification<'_>) -> anyhow::Result<()> {
        let mut errors = 0_usize;

        for notifier in &self.0 {
            if let Err(err) = notifier.notify(notification).await {
                error!("failed to notify with sub-notifier: {err}");
                errors += 1;
            }
        }

        if errors > 0 {
            bail!("{errors} error(s) occurred, see above")
        }
        Ok(())
    }
}

pub fn notifier(params: &config::Notify) -> Box<dyn Notifier> {
    match params {
        config::Notify::Telegram(ps) => Box::new(NotifierVec(
            ps.iter()
                .map(|p| -> Box<dyn Notifier> { Box::new(TelegramNotifier::new(p.clone())) })
                .collect(),
        )),
    }
}

pub async fn notify(notify: &dyn Notifier, notification: &Notification<'_>) {
    info!("notifying notification '{notification}'");
    if let Err(err) = notify.notify(notification).await {
        error!("failed to notify: {err}");
    }
}
