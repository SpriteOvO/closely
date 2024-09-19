pub mod platform;

use std::{future::Future, pin::Pin};

use spdlog::prelude::*;

use crate::{platform::PlatformTrait, source::Notification};

pub trait NotifierTrait: PlatformTrait {
    fn notify<'a>(
        &'a self,
        notification: &'a Notification<'_>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>>;
}

pub fn notifier(params: platform::Config) -> Box<dyn NotifierTrait> {
    match params {
        #[cfg(feature = "qq")]
        platform::Config::Qq(p) => Box::new(platform::qq::Notifier::new(p)),
        platform::Config::Telegram(p) => Box::new(platform::telegram::Notifier::new(p)),
    }
}

pub async fn notify(notify: &dyn NotifierTrait, notification: &Notification<'_>) {
    info!("notifying notification '{notification}'");
    if let Err(err) = notify.notify(notification).await {
        error!(
            "failed to notify to {}: {err}",
            notify.metadata().display_name
        );
    }
}
