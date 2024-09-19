use std::{
    borrow::Cow, collections::HashMap, env, error::Error as StdError, str::FromStr, time::Duration,
};

use anyhow::{anyhow, bail};
use once_cell::sync::OnceCell;
use serde::{de::DeserializeOwned, Deserialize};

use crate::{
    helper, notify,
    reporter::{ConfigReporterRaw, ReporterParams},
    serde_impl_default_for, source,
};

#[derive(Debug, PartialEq, Deserialize)]
pub struct Config {
    #[serde(with = "humantime_serde")]
    pub interval: Duration,
    reporter: Option<ConfigReporterRaw>,
    platform: Option<PlatformGlobal>,
    #[serde(rename = "notify", default)]
    notify_map: NotifyMap,
    subscription: HashMap<String, Vec<SubscriptionRaw>>,
}

static PLATFORM_GLOBAL: OnceCell<PlatformGlobal> = OnceCell::new();

impl Config {
    pub async fn init(input: impl AsRef<str>) -> anyhow::Result<Self> {
        let mut config = Self::from_str(input)?;
        if let Some(reporter) = &config.reporter {
            reporter
                .init(&config.notify_map)
                .map_err(|err| anyhow!("failed to initialize reporter: {err}"))?;
        }
        PLATFORM_GLOBAL
            .set(config.platform.take().unwrap_or_default())
            .map_err(|_| anyhow!("config was initialized before"))?;
        PLATFORM_GLOBAL.get().unwrap().init().await?;
        Ok(config)
    }

    pub fn platform_global() -> &'static PlatformGlobal {
        PLATFORM_GLOBAL.get().expect("config was not initialized")
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

impl Config {
    fn from_str(input: impl AsRef<str>) -> anyhow::Result<Self> {
        let config = toml::from_str::<Self>(input.as_ref())?;
        config
            .validate()
            .map_err(|err| anyhow!("invalid configuration: {err}"))?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        let global = self
            .platform
            .as_ref()
            .map(Cow::Borrowed)
            .unwrap_or(Cow::Owned(PlatformGlobal::default()));

        // Validate platform_global
        if let Some(platform) = &self.platform {
            platform.validate()?;
        }

        // Validate reporter
        if let Some(reporter) = &self.reporter {
            reporter.validate(&self.notify_map)?;
        }

        // Validate source
        self.subscription
            .values()
            .flatten()
            .map(|subscription| &subscription.platform)
            .map(|platform| platform.validate(&global))
            .collect::<Result<Vec<_>, _>>()?;

        // Validate notify ref
        self.subscription
            .values()
            .flatten()
            .flat_map(|subscription| &subscription.notify_ref)
            .map(|notify_ref| self.notify_map.get_by_ref(notify_ref))
            .collect::<Result<Vec<_>, _>>()?;

        // Validate notify_map
        self.notify_map.validate(&global)?;

        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Default, Deserialize)]
pub struct PlatformGlobal {
    #[cfg(feature = "qq")]
    #[serde(rename = "QQ")]
    pub qq: Option<notify::platform::qq::ConfigGlobal>,
    #[serde(rename = "Telegram")]
    pub telegram: Option<notify::platform::telegram::ConfigGlobal>,
    #[serde(rename = "Twitter")]
    pub twitter: Option<source::platform::twitter::ConfigGlobal>,
}

impl PlatformGlobal {
    async fn init(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn validate(&self) -> anyhow::Result<()> {
        if let Some(telegram) = &self.telegram {
            if let Some(token) = &telegram.token {
                token.as_secret_ref().validate()?;
            }
        }
        if let Some(twitter) = &self.twitter {
            twitter.auth.as_secret_ref().validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct SubscriptionRaw {
    pub platform: source::platform::Config,
    #[serde(default, with = "humantime_serde")]
    pub interval: Option<Duration>,
    #[serde(rename = "notify")]
    notify_ref: Vec<NotifyRef>,
}

#[derive(Debug, PartialEq)]
pub struct SubscriptionRef<'a> {
    pub platform: &'a source::platform::Config,
    pub interval: Option<Duration>,
    pub notify: Vec<notify::platform::Config>,
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
pub struct NotifyMap(#[serde(default)] HashMap<String, notify::platform::Config>);

impl NotifyMap {
    pub fn get_by_ref(&self, notify_ref: &NotifyRef) -> anyhow::Result<notify::platform::Config> {
        let original = self
            .0
            .get(notify_ref.name())
            .cloned()
            .ok_or_else(|| anyhow!("reference of notify not found '{}'", notify_ref.name()))?;
        match notify_ref {
            NotifyRef::Direct(_name) => Ok(original),
            NotifyRef::Override { name: _name, new } => original
                .override_into(new.clone())
                .map_err(|err| anyhow!("failed to override notify: {err}")),
        }
    }

    fn validate(&self, global: &PlatformGlobal) -> anyhow::Result<()> {
        self.0
            .values()
            .try_for_each(|notify| notify.validate(global))
    }
}

pub trait AsSecretRef<'a, T = &'a str> {
    fn as_secret_ref(&'a self) -> SecretRef<'_, T>;
}

pub enum SecretRef<'a, T = &'a str> {
    Lit(T),
    Env(&'a str),
}

impl<T> SecretRef<'_, T> {
    pub fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Lit(_) => Ok(()),
            Self::Env(key) => match env::var(key) {
                Ok(_) => Ok(()),
                Err(err) => bail!("{err} ({key})"),
            },
        }
    }
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
                $(#[$attr])* $vis enum $name {
                    $field($type),
                    [<$field Env>](String),
                }
            }
            secret_enum!(@IMPL($type) => $name, $field);
        )+
    };
    ( @IMPL(String) => $name:ident, $field:ident ) => {
        impl $crate::config::AsSecretRef<'_> for $name {
            fn as_secret_ref(&self) -> $crate::config::SecretRef {
                paste::paste! {
                    match self {
                        Self::$field(value) => $crate::config::SecretRef::Lit(value),
                        Self::[<$field Env>](key) => $crate::config::SecretRef::Env(key),
                    }
                }
            }
        }
    };
    ( @IMPL($type:ty) => $name:ident, $field:ident ) => {
        impl $crate::config::AsSecretRef<'_, $type> for $name {
            fn as_secret_ref(&self) -> $crate::config::SecretRef<'_, $type> {
                paste::paste! {
                    match self {
                        Self::$field(value) => $crate::config::SecretRef::Lit(*value),
                        Self::[<$field Env>](key) => $crate::config::SecretRef::Env(key),
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
            Config::from_str(
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
            Config {
                interval: Duration::from_secs(60), // 1min
                reporter: Some(ConfigReporterRaw {
                    log: Some(ConfigReporterLog {
                        notify_ref: vec![NotifyRef::Direct("meow".into())],
                    }),
                    heartbeat: Some(ConfigHeartbeat {
                        kind: ConfigHeartbeatKind::HttpGet(ConfigHeartbeatHttpGet {
                            url: "https://example.com/".into(),
                        }),
                        interval: Duration::from_secs(60),
                    }),
                }),
                platform: Some(PlatformGlobal {
                    qq: Some(notify::platform::qq::ConfigGlobal {
                        lagrange: notify::platform::qq::lagrange::ConfigLagrange {
                            http_host: "localhost".into(),
                            http_port: 8000,
                            access_token: None,
                        }
                    }),
                    telegram: Some(notify::platform::telegram::ConfigGlobal {
                        token: Some(notify::platform::telegram::ConfigToken::Token("ttt".into())),
                        experimental: Default::default()
                    }),
                    twitter: Some(source::platform::twitter::ConfigGlobal {
                        auth: source::platform::twitter::ConfigCookies::Cookies("a=b;c=d;ct0=blah".into())
                    })
                }),
                notify_map: NotifyMap(HashMap::from_iter([
                    (
                        "meow".into(),
                        notify::platform::Config::Telegram(notify::platform::telegram::ConfigParams {
                            notifications: Notifications::default(),
                            chat: notify::platform::telegram::ConfigChat::Id(1234),
                            thread_id: Some(123),
                            token: Some(notify::platform::telegram::ConfigToken::Token("xxx".into())),
                        })
                    ),
                    (
                        "woof".into(),
                        notify::platform::Config::Telegram(notify::platform::telegram::ConfigParams {
                            notifications: Notifications {
                                live_online: true,
                                live_title: false,
                                post: false,
                                log: true,
                            },
                            chat: notify::platform::telegram::ConfigChat::Id(5678),
                            thread_id: Some(900),
                            token: None,
                        })
                    )
                ])),
                subscription: HashMap::from_iter([(
                    "meow".into(),
                    vec![
                        SubscriptionRaw {
                            platform: source::platform::Config::BilibiliLive(
                                source::platform::bilibili::live::ConfigParams { user_id: 123456 }
                            ),
                            interval: Some(Duration::from_secs(30)),
                            notify_ref: vec![NotifyRef::Direct("meow".into())],
                        },
                        SubscriptionRaw {
                            platform: source::platform::Config::Twitter(
                                source::platform::twitter::ConfigParams {
                                    username: "meowww".into()
                                }
                            ),
                            interval: None,
                            notify_ref: vec![
                                NotifyRef::Direct("meow".into()),
                                NotifyRef::Direct("woof".into())
                            ],
                        },
                        SubscriptionRaw {
                            platform: source::platform::Config::Twitter(
                                source::platform::twitter::ConfigParams {
                                    username: "meowww2".into()
                                }
                            ),
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

        assert!(Config::from_str(
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

        assert!(Config::from_str(
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

        assert!(Config::from_str(
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

        assert!(Config::from_str(
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

        assert!(Config::from_str(
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
        let config = Config::from_str(
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
                    platform: &source::platform::Config::BilibiliLive(
                        source::platform::bilibili::live::ConfigParams { user_id: 123456 }
                    ),
                    interval: None,
                    notify: vec![
                        notify::platform::Config::Telegram(
                            notify::platform::telegram::ConfigParams {
                                notifications: Notifications::default(),
                                chat: notify::platform::telegram::ConfigChat::Id(1234),
                                thread_id: Some(123),
                                token: Some(notify::platform::telegram::ConfigToken::Token(
                                    "xxx".into()
                                )),
                            }
                        ),
                        notify::platform::Config::Telegram(
                            notify::platform::telegram::ConfigParams {
                                notifications: Notifications::default(),
                                chat: notify::platform::telegram::ConfigChat::Id(5678),
                                thread_id: Some(114),
                                token: Some(notify::platform::telegram::ConfigToken::Token(
                                    "yyy".into()
                                )),
                            }
                        ),
                        notify::platform::Config::Telegram(
                            notify::platform::telegram::ConfigParams {
                                notifications: Notifications {
                                    post: false,
                                    ..Default::default()
                                },
                                chat: notify::platform::telegram::ConfigChat::Id(5678),
                                thread_id: Some(456),
                                token: Some(notify::platform::telegram::ConfigToken::Token(
                                    "yyy".into()
                                )),
                            }
                        )
                    ],
                }
            ),]
        );
    }
}
