mod telegram;

use spdlog::prelude::*;

use crate::{config::Notify, platform::LiveStatus};

pub async fn notify(notify: &Notify, live_status: &LiveStatus) {
    trace!("notify '{notify}' with live status '{live_status}'");

    match notify {
        Notify::Telegram(ns) => {
            for n in ns {
                if let Err(err) = telegram::notify(n, live_status).await {
                    error!("failed to notify '{n}' with live status: '{live_status:?}': {err}");
                }
            }
        }
    }
}
