use std::{
    borrow::Cow,
    collections::HashMap,
    env,
    error::Error as StdError,
    fmt::Display,
    ops,
    str::FromStr,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use anyhow::anyhow;
use serde::{de::DeserializeOwned, Deserialize};
use spdlog::prelude::*;

use crate::{
    helper, notify,
    reporter::{ConfigReporterRaw, ReporterParams},
    serde_impl_default_for, source,
};

pub trait Validator {
    fn validate(&self) -> anyhow::Result<()>;
}

impl<T: Validator> Validator for Option<T> {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(data) = self {
            data.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(transparent)]
pub struct Accessor<T> {
    #[serde(skip)]
    is_validated: AtomicBool,
    #[serde(flatten)]
    data: T,
}

impl<T: Clone> Clone for Accessor<T> {
    fn clone(&self) -> Self {
        Self {
            is_validated: AtomicBool::new(self.is_validated()),
            data: self.data.clone(),
        }
    }
}

impl<T: PartialEq> PartialEq for Accessor<T> {
    fn eq(&self, other: &Self) -> bool {
        self.data.eq(&other.data)
    }
}

impl<T: Display> Display for Accessor<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.data.fmt(f)
    }
}

impl<T> Accessor<T> {
    pub fn new(data: T) -> Self {
        Self {
            is_validated: AtomicBool::new(false),
            data,
        }
    }

    pub fn is_validated(&self) -> bool {
        self.is_validated.load(Ordering::Relaxed)
    }

    fn ensure_invalidated(&self) {
        if !self.is_validated() {
            panic!("config accessed before validation");
        }
    }

    pub fn into_inner(self) -> T {
        self.ensure_invalidated();
        self.data
    }
}

impl<T: Validator> Accessor<T> {
    pub fn new_then_validate(data: T) -> anyhow::Result<Self> {
        let accessor = Self::new(data);
        accessor.validate().map(|_| accessor)
    }
}

impl<T: Validator> Validator for Accessor<T> {
    fn validate(&self) -> anyhow::Result<()> {
        if !self.is_validated() {
            self.data.validate()?;
            self.is_validated.store(true, Ordering::Relaxed);
        } else {
            trace!("config validated multiple times");
        }
        Ok(())
    }
}

impl<T: Validator> ops::Deref for Accessor<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.ensure_invalidated();
        &self.data
    }
}

impl<T: Validator> ops::DerefMut for Accessor<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.ensure_invalidated();
        &mut self.data
    }
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct Config {
    #[serde(with = "humantime_serde")]
    pub interval: Duration,
    reporter: Accessor<Option<ConfigReporterRaw>>,
    #[serde(default)]
    platform: Accessor<PlatformGlobal>,
    #[serde(rename = "notify", default)]
    notify_map: Accessor<NotifyMap>,
    subscription: HashMap<String, Vec<SubscriptionRaw>>,
}

#[cfg(not(test))]
static CONFIG: once_cell::sync::OnceCell<Config> = once_cell::sync::OnceCell::new();
#[cfg(test)]
static CONFIG: std::sync::Mutex<Option<std::sync::Arc<Config>>> = std::sync::Mutex::new(None);

impl Config {
    pub async fn init(input: impl AsRef<str>) -> anyhow::Result<&'static Self> {
        let config = toml::from_str::<Self>(input.as_ref())?;

        #[cfg(not(test))]
        CONFIG
            .set(config)
            .map_err(|_| anyhow!("config was initialized before"))?;
        #[cfg(test)]
        drop(config); // Suppress the warning of unused variable

        let config = Self::global();
        config
            .validate()
            .map_err(|err| anyhow!("invalid configuration: {err}"))?;
        if let Some(reporter) = &*config.reporter {
            reporter
                .init(&config.notify_map)
                .map_err(|err| anyhow!("failed to initialize reporter: {err}"))?;
        }
        Ok(config)
    }

    #[cfg(test)]
    fn from_str_for_test(input: impl AsRef<str>) -> anyhow::Result<&'static Self> {
        let config = toml::from_str::<Self>(input.as_ref())?;
        *CONFIG.lock().unwrap() = Some(std::sync::Arc::new(config));

        Self::global()
            .validate()
            .map_err(|err| anyhow!("invalid configuration: {err}"))?;
        Ok(Self::global())
    }

    pub fn global() -> &'static Self {
        #[cfg(not(test))]
        let ret = &CONFIG.get().expect("config was not initialized");
        #[cfg(test)]
        let ret = Box::leak(Box::new(CONFIG.lock().unwrap().clone().unwrap()));
        ret
    }

    pub fn platform(&self) -> &Accessor<PlatformGlobal> {
        &self.platform
    }

    pub fn notify_map(&self) -> &Accessor<NotifyMap> {
        &self.notify_map
    }

    pub fn subscriptions(&self) -> impl Iterator<Item = (String, SubscriptionRef<'_>)> {
        self.subscription.iter().flat_map(|(name, subscriptions)| {
            subscriptions.iter().map(|subscription| {
                (
                    name.clone(),
                    SubscriptionRef {
                        platform: &subscription.platform,
                        interval: subscription.interval,
                        notify: subscription
                            .notify_ref
                            .iter()
                            .map(|notify_ref| self.notify_map.get_by_ref(notify_ref).unwrap())
                            .collect(),
                    },
                )
            })
        })
    }

    pub fn reporter(&self) -> Option<ReporterParams> {
        self.reporter.as_ref().map(|r| r.reporter())
    }
}

impl Validator for Config {
    fn validate(&self) -> anyhow::Result<()> {
        // Validate reporter
        self.platform.validate()?;

        // Validate notify_map
        self.notify_map.validate()?;

        // Validate reporter
        self.reporter.validate()?;

        // Validate source
        self.subscription
            .values()
            .flatten()
            .map(|subscription| &subscription.platform)
            .map(|platform| platform.validate())
            .collect::<Result<Vec<_>, _>>()?;

        // Validate notify ref
        self.subscription
            .values()
            .flatten()
            .flat_map(|subscription| &subscription.notify_ref)
            .map(|notify_ref| self.notify_map.get_by_ref(notify_ref))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Default, Deserialize)]
pub struct PlatformGlobal {
    #[cfg(feature = "qq")]
    #[serde(rename = "QQ")]
    pub qq: Accessor<Option<notify::platform::qq::ConfigGlobal>>,
    #[serde(rename = "Telegram")]
    pub telegram: Accessor<Option<notify::platform::telegram::ConfigGlobal>>,
    #[serde(rename = "Twitter")]
    pub twitter: Accessor<Option<source::platform::twitter::ConfigGlobal>>,
}

impl Validator for PlatformGlobal {
    fn validate(&self) -> anyhow::Result<()> {
        #[cfg(feature = "qq")]
        self.qq.validate()?;
        self.telegram.validate()?;
        self.twitter.validate()?;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct SubscriptionRaw {
    pub platform: Accessor<source::platform::Config>,
    #[serde(default, with = "humantime_serde")]
    pub interval: Option<Duration>,
    #[serde(rename = "notify")]
    notify_ref: Vec<NotifyRef>,
}

#[derive(Debug, PartialEq)]
pub struct SubscriptionRef<'a> {
    pub platform: &'a Accessor<source::platform::Config>,
    pub interval: Option<Duration>,
    pub notify: Vec<Accessor<notify::platform::Config>>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Notifications {
    #[serde(default = "helper::refl_bool::<true>")]
    pub live_online: bool,
    #[serde(default = "helper::refl_bool::<false>")]
    pub live_title: bool,
    #[serde(default = "helper::refl_bool::<true>")]
    pub post: bool,
    #[serde(default = "helper::refl_bool::<true>")]
    pub log: bool,
}

serde_impl_default_for!(Notifications);

impl Overridable for Notifications {
    type Override = NotificationsOverride;

    fn override_into(self, new: Self::Override) -> Self
    where
        Self: Sized,
    {
        Self {
            live_online: new.live_online.unwrap_or(self.live_online),
            live_title: new.live_title.unwrap_or(self.live_title),
            post: new.post.unwrap_or(self.post),
            log: new.log.unwrap_or(self.log),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct NotificationsOverride {
    pub live_online: Option<bool>,
    pub live_title: Option<bool>,
    pub post: Option<bool>,
    pub log: Option<bool>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum NotifyRef {
    Direct(String),
    Override {
        #[serde(rename = "ref")]
        name: String,
        #[serde(flatten)]
        new: toml::Value,
    },
}

impl NotifyRef {
    fn name(&self) -> &str {
        match self {
            NotifyRef::Direct(name) => name,
            NotifyRef::Override { name, .. } => name,
        }
    }
}

#[derive(Debug, Default, PartialEq, Deserialize)]
pub struct NotifyMap(#[serde(default)] HashMap<String, Accessor<notify::platform::Config>>);

impl Validator for NotifyMap {
    fn validate(&self) -> anyhow::Result<()> {
        self.0.values().try_for_each(|notify| notify.validate())
    }
}

impl NotifyMap {
    pub fn get_by_ref(
        &self,
        notify_ref: &NotifyRef,
    ) -> anyhow::Result<Accessor<notify::platform::Config>> {
        let original = self
            .0
            .get(notify_ref.name())
            .cloned()
            .ok_or_else(|| anyhow!("reference of notify not found '{}'", notify_ref.name()))?;
        match notify_ref {
            NotifyRef::Direct(_name) => Ok(original),
            NotifyRef::Override { name: _name, new } => original
                .into_inner()
                .override_into(new.clone())
                .map(Accessor::new_then_validate)
                .map_err(|err| anyhow!("failed to override notify: {err}"))?,
        }
    }
}

pub trait AsSecretRef<'a, T = &'a str> {
    fn as_secret_ref(&'a self) -> SecretRef<'a, T>;
}

pub enum SecretRef<'a, T = &'a str> {
    Lit(T),
    Env(&'a str),
}

impl<'a> SecretRef<'a, &'a str> {
    pub fn get_str(&self) -> anyhow::Result<Cow<'a, str>> {
        match self {
            Self::Lit(lit) => Ok(Cow::Borrowed(lit)),
            Self::Env(key) => Ok(Cow::Owned(env::var(key)?)),
        }
    }
}

impl<T: Copy + FromStr> SecretRef<'_, T>
where
    <T as FromStr>::Err: StdError + Send + Sync + 'static,
{
    pub fn get_parse_copy(&self) -> anyhow::Result<T> {
        match self {
            Self::Lit(lit) => Ok(*lit),
            Self::Env(key) => Ok(env::var(key)?.parse()?),
        }
    }
}

impl<T: ToOwned> SecretRef<'_, T>
where
    <T as ToOwned>::Owned: FromStr,
    <<T as ToOwned>::Owned as FromStr>::Err: StdError + Send + Sync + 'static,
{
    pub fn get_parse_cow(&self) -> anyhow::Result<Cow<T>> {
        match self {
            Self::Lit(lit) => Ok(Cow::Borrowed(lit)),
            Self::Env(key) => Ok(Cow::Owned(env::var(key)?.parse()?)),
        }
    }
}

#[macro_export]
macro_rules! secret_enum {
    ( $($(#[$attr:meta])* $vis:vis enum $name:ident { $field:ident($type:ident)$(,)? })+ ) => {
        $(
            paste::paste! {
                mod secret_enum_private {
                    use super::*;

                    $(#[$attr])* pub enum $name {
                        $field($type),
                        [<$field Env>](String),
                    }

                    impl $crate::config::Validator for $name {
                        fn validate(&self) -> anyhow::Result<()> {
                            match self {
                                Self::$field(_) => Ok(()),
                                Self::[<$field Env>](key) => match std::env::var(key) {
                                    Ok(_) => Ok(()),
                                    Err(err) => bail!("{err} ({key})"),
                                },
                            }
                        }
                    }
                }

                $(#[$attr])* $vis struct $name($crate::config::Accessor<secret_enum_private::$name>);

                impl $name {
                    pub fn with_env(key: impl Into<String>) -> Self {
                        paste::paste! {
                            Self($crate::config::Accessor::new(secret_enum_private::$name::[<$field Env>](key.into())))
                        }
                    }
                }

                impl $crate::config::Validator for $name {
                    fn validate(&self) -> anyhow::Result<()> {
                        self.0.validate()
                    }
                }
            }
            secret_enum!(@IMPL($type) => $name, $field);
        )+
    };
    ( @IMPL(String) => $name:ident, $field:ident ) => {
        impl $name {
            pub fn with_raw(raw: impl Into<String>) -> Self {
                paste::paste! {
                    Self($crate::config::Accessor::new(secret_enum_private::$name::$field(raw.into())))
                }
            }
        }

        impl $crate::config::AsSecretRef<'_> for $name {
            fn as_secret_ref(&self) -> $crate::config::SecretRef {
                paste::paste! {
                    match &*self.0 {
                        secret_enum_private::$name::$field(value) => $crate::config::SecretRef::Lit(value),
                        secret_enum_private::$name::[<$field Env>](key) => $crate::config::SecretRef::Env(key),
                    }
                }
            }
        }
    };
    ( @IMPL($type:ty) => $name:ident, $field:ident ) => {
        impl $name {
            pub fn with_raw(raw: impl Into<$type>) -> Self {
                paste::paste! {
                    Self(secret_enum_private::$name::$field(raw.into()))
                }
            }
        }

        impl $crate::config::AsSecretRef<'_, $type> for $name {
            fn as_secret_ref(&self) -> $crate::config::SecretRef<'_, $type> {
                paste::paste! {
                    match &self.0 {
                        secret_enum_private::$name::$field(value) => $crate::config::SecretRef::Lit(*value),
                        secret_enum_private::$name::[<$field Env>](key) => $crate::config::SecretRef::Env(key),
                    }
                }
            }
        }
    };
}

pub trait Overridable {
    type Override: DeserializeOwned;

    fn override_into(self, new: Self::Override) -> Self
    where
        Self: Sized;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reporter::{
        ConfigHeartbeat, ConfigHeartbeatHttpGet, ConfigHeartbeatKind, ConfigReporterLog,
    };

    #[test]
    fn deser() {
        assert_eq!(
            Config::from_str_for_test(
                r#"
interval = '1min'
reporter = { log = { notify = ["meow"] }, heartbeat = { type = "HttpGet", url = "https://example.com/", interval = '1min' } } 

[platform.QQ]
lagrange = { http_host = "localhost", http_port = 8000 }

[platform.Telegram]
token = "ttt"

[platform.Twitter]
auth = { cookies = "a=b;c=d;ct0=blah" }

[notify]
meow = { platform = "Telegram", id = 1234, thread_id = 123, token = "xxx" }
woof = { platform = "Telegram", id = 5678, thread_id = 900, notifications = { post = false } }

[[subscription.meow]]
platform = { name = "bilibili.live", user_id = 123456 }
interval = '30s'
notify = ["meow"]

[[subscription.meow]]
platform = { name = "Twitter", username = "meowww" }
notify = ["meow", "woof"]

[[subscription.meow]]
platform = { name = "Twitter", username = "meowww2" }
notify = ["meow", "woof", { ref = "woof", id = 123 }]
                "#,
            )
            .unwrap(),
            &Config {
                interval: Duration::from_secs(60), // 1min
                reporter: Accessor::new(Some(ConfigReporterRaw {
                    log: Accessor::new(Some(ConfigReporterLog {
                        notify_ref: vec![NotifyRef::Direct("meow".into())],
                    })),
                    heartbeat: Accessor::new(Some(ConfigHeartbeat {
                        kind: ConfigHeartbeatKind::HttpGet(ConfigHeartbeatHttpGet {
                            url: "https://example.com/".into(),
                        }),
                        interval: Duration::from_secs(60),
                    })),
                })),
                platform: Accessor::new(PlatformGlobal {
                    qq: Accessor::new(Some(notify::platform::qq::ConfigGlobal {
                        lagrange: notify::platform::qq::lagrange::ConfigLagrange {
                            http_host: "localhost".into(),
                            http_port: 8000,
                            access_token: None,
                        }
                    })),
                    telegram: Accessor::new(Some(notify::platform::telegram::ConfigGlobal {
                        token: Some(notify::platform::telegram::ConfigToken::with_raw("ttt")),
                        experimental: Default::default()
                    })),
                    twitter: Accessor::new(Some(source::platform::twitter::ConfigGlobal {
                        auth: source::platform::twitter::ConfigCookies::with_raw("a=b;c=d;ct0=blah")
                    }))
                }),
                notify_map: Accessor::new(NotifyMap(HashMap::from_iter([
                    (
                        "meow".into(),
                        Accessor::new(notify::platform::Config::Telegram(Accessor::new(notify::platform::telegram::ConfigParams {
                            notifications: Notifications::default(),
                            chat: notify::platform::telegram::ConfigChat::Id(1234),
                            thread_id: Some(123),
                            token: Some(notify::platform::telegram::ConfigToken::with_raw("xxx")),
                        })))
                    ),
                    (
                        "woof".into(),
                        Accessor::new(notify::platform::Config::Telegram(Accessor::new(notify::platform::telegram::ConfigParams {
                            notifications: Notifications {
                                live_online: true,
                                live_title: false,
                                post: false,
                                log: true,
                            },
                            chat: notify::platform::telegram::ConfigChat::Id(5678),
                            thread_id: Some(900),
                            token: None,
                        })))
                    )
                ]))),
                subscription: HashMap::from_iter([(
                    "meow".into(),
                    vec![
                        SubscriptionRaw {
                            platform: Accessor::new(source::platform::Config::BilibiliLive(
                                Accessor::new(source::platform::bilibili::live::ConfigParams { user_id: 123456 })
                            )),
                            interval: Some(Duration::from_secs(30)),
                            notify_ref: vec![NotifyRef::Direct("meow".into())],
                        },
                        SubscriptionRaw {
                            platform: Accessor::new(source::platform::Config::Twitter(
                                Accessor::new(source::platform::twitter::ConfigParams {
                                    username: "meowww".into()
                                })
                            )),
                            interval: None,
                            notify_ref: vec![
                                NotifyRef::Direct("meow".into()),
                                NotifyRef::Direct("woof".into())
                            ],
                        },
                        SubscriptionRaw {
                            platform: Accessor::new(source::platform::Config::Twitter(
                                Accessor::new(source::platform::twitter::ConfigParams {
                                    username: "meowww2".into()
                                })
                            )),
                            interval: None,
                            notify_ref: vec![
                                NotifyRef::Direct("meow".into()),
                                NotifyRef::Direct("woof".into()),
                                NotifyRef::Override {
                                    name: "woof".into(),
                                    new: toml::Value::Table(toml::Table::from_iter([(
                                        "id".into(),
                                        toml::Value::Integer(123)
                                    )]))
                                }
                            ],
                        }
                    ]
                )]),
            }
        );

        assert!(Config::from_str_for_test(
            r#"
interval = '1min'
reporter = { notify = ["meow"], heartbeat = { type = "HttpGet", url = "https://example.com/", interval = '1min' } } 

[notify]
meow = { platform = "Telegram", id = 1234, thread_id = 123, token = "xxx" }

[[subscription.meow]]
platform = { name = "bilibili.live", user_id = 123456 }
notify = ["meow"]
                "#
        )
        .is_ok());

        assert!(Config::from_str_for_test(
            r#"
interval = '1min'
reporter = { log = { notify = ["reporter_notify"] }, heartbeat = { type = "HttpGet", url = "https://example.com/", interval = '1min' } } 

[[subscription.meow]]
platform = { name = "bilibili.live", user_id = 123456 }
notify = []
            "#
        )
        .unwrap_err()
        .to_string()
        .ends_with("reference of notify not found 'reporter_notify'"));

        assert!(Config::from_str_for_test(
            r#"
interval = '1min'

[[subscription.meow]]
platform = { name = "bilibili.live", user_id = 123456 }
notify = ["meow"]
                "#
        )
        .unwrap_err()
        .to_string()
        .ends_with("reference of notify not found 'meow'"));

        assert!(Config::from_str_for_test(
            r#"
interval = '1min'

[notify]
meow = { platform = "Telegram", id = 1234, thread_id = 123, token = "xxx" }

[[subscription.meow]]
platform = { name = "bilibili.live", user_id = 123456 }
notify = ["meow", "woof"]
                "#
        )
        .unwrap_err()
        .to_string()
        .ends_with("reference of notify not found 'woof'"));

        assert!(Config::from_str_for_test(
            r#"
interval = '1min'

[notify]
meow = { platform = "Telegram", id = 1234, thread_id = 123 }

[[subscription.meow]]
platform = { name = "bilibili.live", user_id = 123456 }
notify = ["meow"]
                "#
        )
        .unwrap_err()
        .to_string()
        .ends_with("both token in global and notify are missing"));
    }

    #[test]
    fn option_override() {
        let config = Config::from_str_for_test(
            r#"
interval = '1min'

[notify]
meow = { platform = "Telegram", id = 1234, thread_id = 123, token = "xxx" }
woof = { platform = "Telegram", id = 5678, thread_id = 456, token = "yyy" }

[[subscription.meow]]
platform = { name = "bilibili.live", user_id = 123456 }
notify = ["meow", { ref = "woof", thread_id = 114 }, { ref = "woof", notifications = { post = false } }]
                "#,
        )
        .unwrap();

        let subscriptions = config.subscriptions().collect::<Vec<_>>();

        assert_eq!(
            subscriptions,
            vec![(
                "meow".into(),
                SubscriptionRef {
                    platform: &Accessor::new(source::platform::Config::BilibiliLive(
                        Accessor::new(source::platform::bilibili::live::ConfigParams {
                            user_id: 123456
                        })
                    )),
                    interval: None,
                    notify: vec![
                        Accessor::new(notify::platform::Config::Telegram(Accessor::new(
                            notify::platform::telegram::ConfigParams {
                                notifications: Notifications::default(),
                                chat: notify::platform::telegram::ConfigChat::Id(1234),
                                thread_id: Some(123),
                                token: Some(notify::platform::telegram::ConfigToken::with_raw(
                                    "xxx"
                                )),
                            }
                        ))),
                        Accessor::new(notify::platform::Config::Telegram(Accessor::new(
                            notify::platform::telegram::ConfigParams {
                                notifications: Notifications::default(),
                                chat: notify::platform::telegram::ConfigChat::Id(5678),
                                thread_id: Some(114),
                                token: Some(notify::platform::telegram::ConfigToken::with_raw(
                                    "yyy"
                                )),
                            }
                        ))),
                        Accessor::new(notify::platform::Config::Telegram(Accessor::new(
                            notify::platform::telegram::ConfigParams {
                                notifications: Notifications {
                                    post: false,
                                    ..Default::default()
                                },
                                chat: notify::platform::telegram::ConfigChat::Id(5678),
                                thread_id: Some(456),
                                token: Some(notify::platform::telegram::ConfigToken::with_raw(
                                    "yyy"
                                )),
                            }
                        )))
                    ],
                }
            ),]
        );
    }
}
