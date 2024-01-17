mod telegram;

use spdlog::prelude::*;

use crate::{config::Notify, platform::Notification};

pub async fn notify(notify: &Notify, notification: &Notification<'_>) {
    trace!("notify '{notify}' with notification '{notification}'");

    match notify {
        Notify::Telegram(ns) => {
            for n in ns {
                if let Err(err) = telegram::notify(n, notification).await {
                    error!("failed to notify '{n}' with notification '{notification:?}': {err}");
                }
            }
        }
    }
}
