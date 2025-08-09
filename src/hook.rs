use once_cell::sync::OnceCell;

use crate::{
    notify,
    platform::PlatformTrait,
    source::{LiveStatus, Notification, StatusSource},
};

pub(crate) static HOOKS: OnceCell<Option<Hooks>> = OnceCell::new();

pub enum HookParamsLiveText<'a> {
    Telegram(
        &'a platform::telegram::notify::ConfigParams,
        &'a mut platform::telegram::notify::HookParamsLiveText,
    ),
    Qq(
        &'a platform::qq::notify::ConfigParams,
        &'a mut platform::qq::notify::HookParamsLiveText,
    ),
}

#[derive(Default)]
pub struct Hooks {
    // Called when any new notifications are generated.
    pub notifications_generated:
        Option<Box<dyn Fn(&dyn PlatformTrait, &mut Vec<Notification>) + Send + Sync>>,

    // Called before any notification will be notified.
    pub live_text: Option<
        Box<
            dyn for<'a> Fn(
                    &dyn PlatformTrait,
                    &mut HookParamsLiveText<'_>,
                    &LiveStatus,
                    &StatusSource,
                    Box<dyn FnOnce() -> String + 'a>,
                ) -> String
                + Send
                + Sync,
        >,
    >,
}

pub(crate) fn notifications_generated<'a>(
    sourcer: &dyn PlatformTrait,
    notifications: &mut Vec<Notification<'_>>,
) {
    if let Some(callback) = HOOKS
        .get()
        .unwrap()
        .as_ref()
        .and_then(|hooks| hooks.notifications_generated.as_ref())
    {
        callback(sourcer.into(), notifications)
    }
}

pub(crate) fn live_text<'a, 'b>(
    notifier: &dyn PlatformTrait,
    params: impl Into<HookParamsLiveText<'a>>,
    live_status: &LiveStatus,
    source: &StatusSource,
    default: impl FnOnce() -> String + 'b,
) -> String {
    if let Some(callback) = HOOKS
        .get()
        .unwrap()
        .as_ref()
        .and_then(|hooks| hooks.live_text.as_ref())
    {
        callback(
            notifier,
            &mut params.into(),
            live_status,
            source,
            Box::new(default),
        )
    } else {
        default()
    }
}
