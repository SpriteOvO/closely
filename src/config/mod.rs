mod overridable;
mod secret;
mod validator;

use std::{collections::HashMap, time::Duration};

use anyhow::anyhow;
pub use overridable::*;
pub use secret::*;
use serde::Deserialize;
pub use validator::*;

use crate::{
    helper,
    notify::NotifierConfig,
    platform::*,
    reporter::{ConfigReporterRaw, ReporterParams},
    serde_impl_default_for,
    source::SourceConfig,
};

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
static CONFIG: parking_lot::RwLock<Option<std::sync::Arc<Config>>> = parking_lot::RwLock::new(None);

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
    fn parse_for_test(input: impl AsRef<str>, cb: impl FnOnce(anyhow::Result<&Config>)) {
        let config = toml::from_str::<Self>(input.as_ref()).map_err(anyhow::Error::from);
        match config {
            Ok(config) => {
                let mut write_guard = CONFIG.write();
                *write_guard = Some(std::sync::Arc::new(config));
                let read_guard = parking_lot::RwLockWriteGuard::downgrade(write_guard);
                cb(read_guard
                    .as_ref()
                    .unwrap()
                    .validate()
                    .map_err(|err| anyhow!("invalid configuration: {err}"))
                    .map(|_| &**read_guard.as_ref().unwrap()))
            }
            Err(err) => cb(Err(err)),
        }
    }

    pub fn global() -> &'static Self {
        #[cfg(not(test))]
        let ret = &CONFIG.get().expect("config was not initialized");
        #[cfg(test)]
        let ret = Box::leak(Box::new(CONFIG.read().clone().unwrap()));
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
    #[serde(rename = "QQ")]
    pub qq: Accessor<Option<qq::ConfigGlobal>>,
    #[serde(rename = "Telegram")]
    pub telegram: Accessor<Option<telegram::ConfigGlobal>>,
    #[serde(rename = "Twitter")]
    pub twitter: Accessor<Option<twitter::ConfigGlobal>>,
    #[serde(rename = "bilibili")]
    pub bilibili: Accessor<Option<bilibili::ConfigGlobal>>,
}

impl Validator for PlatformGlobal {
    fn validate(&self) -> anyhow::Result<()> {
        self.qq.validate()?;
        self.telegram.validate()?;
        self.twitter.validate()?;
        self.bilibili.validate()?;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct SubscriptionRaw {
    pub platform: Accessor<SourceConfig>,
    #[serde(default, with = "humantime_serde")]
    pub interval: Option<Duration>,
    #[serde(rename = "notify")]
    notify_ref: Vec<NotifyRef>,
}

#[derive(Debug, PartialEq)]
pub struct SubscriptionRef<'a> {
    pub platform: &'a Accessor<SourceConfig>,
    pub interval: Option<Duration>,
    pub notify: Vec<Accessor<NotifierConfig>>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Notifications {
    // Toggles
    #[serde(default = "helper::refl_bool::<true>")]
    pub live_online: bool,
    #[serde(default = "helper::refl_bool::<false>")]
    pub live_title: bool,
    #[serde(default = "helper::refl_bool::<true>")]
    pub post: bool,
    #[serde(default = "helper::refl_bool::<true>")]
    pub log: bool,
    #[serde(default = "helper::refl_bool::<true>")]
    pub playback: bool,
    #[serde(default = "helper::refl_bool::<true>")]
    pub document: bool,

    // Options
    #[serde(default = "helper::refl_bool::<false>")]
    pub author_name: bool,
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
            playback: new.playback.unwrap_or(self.playback),
            document: new.document.unwrap_or(self.document),
            author_name: new.author_name.unwrap_or(self.author_name),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct NotificationsOverride {
    pub live_online: Option<bool>,
    pub live_title: Option<bool>,
    pub post: Option<bool>,
    pub log: Option<bool>,
    pub playback: Option<bool>,
    pub document: Option<bool>,
    pub author_name: Option<bool>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum NotifyRef {
    Direct(String),
    Override {
        #[serde(rename = "to", alias = "ref")]
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
pub struct NotifyMap(#[serde(default)] HashMap<String, Accessor<NotifierConfig>>);

impl Validator for NotifyMap {
    fn validate(&self) -> anyhow::Result<()> {
        self.0.values().try_for_each(|notify| notify.validate())
    }
}

impl NotifyMap {
    pub fn get_by_ref(&self, notify_ref: &NotifyRef) -> anyhow::Result<Accessor<NotifierConfig>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reporter::{
        ConfigHeartbeat, ConfigHeartbeatHttpGet, ConfigHeartbeatKind, ConfigReporterLog,
    };

    #[test]
    fn deser() {
        Config::parse_for_test(
            r#"
interval = '1min'
reporter = { log = { notify = ["meow"] }, heartbeat = { type = "HttpGet", url = "https://example.com/", interval = '1min' } } 

[platform.QQ.account.MyQQ]
lagrange = { remote_http = { host = "localhost", port = 8000 } }

[platform.Telegram]
token = "ttt"

[platform.Twitter]
auth = { cookies = "a=b;c=d;ct0=blah" }

[platform.bilibili]
playback = { bililive_recorder = { listen_webhook = { host = "127.0.0.1", port = 8888 }, working_directory = "/brec/" } }

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
            |c| {
                assert_eq!(c.unwrap(), &Config {
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
                        qq: Accessor::new(Some(qq::ConfigGlobal {
                            account: HashMap::from_iter([
                                ("MyQQ".into(), Accessor::new(qq::ConfigAccount {
                                    lagrange: qq::lagrange::ConfigLagrange {
                                        remote_http: qq::lagrange::RemoteHttp {
                                            host: "localhost".into(),
                                            port: 8000,
                                        },
                                        access_token: None,
                                    }
                                }))
                            ])
                        })),
                        telegram: Accessor::new(Some(telegram::ConfigGlobal {
                            token: Some(telegram::ConfigToken::with_raw("ttt")),
                            api_server: None,
                            experimental: Default::default()
                        })),
                        twitter: Accessor::new(Some(twitter::ConfigGlobal {
                            auth: twitter::ConfigCookies::with_raw("a=b;c=d;ct0=blah")
                        })),
                        bilibili: Accessor::new(Some(bilibili::ConfigGlobal {
                            playback: Accessor::new(Some(bilibili::source::playback::ConfigGlobal {
                                bililive_recorder: Accessor::new(bilibili::source::playback::bililive_recorder::ConfigBililiveRecorder {
                                    listen_webhook: bilibili::source::playback::bililive_recorder::ConfigListen {
                                        host: "127.0.0.1".into(),
                                        port: 8888
                                    },
                                    working_directory: "/brec/".into()
                                })
                            }))
                        })),
                    }),
                    notify_map: Accessor::new(NotifyMap(HashMap::from_iter([
                        (
                            "meow".into(),
                            Accessor::new(NotifierConfig::Telegram(Accessor::new(telegram::notify::ConfigParams {
                                notifications: Notifications::default(),
                                chat: telegram::ConfigChat::Id(1234),
                                thread_id: Some(123),
                                token: Some(telegram::ConfigToken::with_raw("xxx")),
                            })))
                        ),
                        (
                            "woof".into(),
                            Accessor::new(NotifierConfig::Telegram(Accessor::new(telegram::notify::ConfigParams {
                                notifications: Notifications {
                                    live_online: true,
                                    live_title: false,
                                    post: false,
                                    log: true,
                                    playback: true,
                                    document: true,
                                    author_name: false,
                                },
                                chat: telegram::ConfigChat::Id(5678),
                                thread_id: Some(900),
                                token: None,
                            })))
                        )
                    ]))),
                    subscription: HashMap::from_iter([(
                        "meow".into(),
                        vec![
                            SubscriptionRaw {
                                platform: Accessor::new(SourceConfig::BilibiliLive(
                                    Accessor::new(bilibili::source::live::ConfigParams { user_id: 123456 })
                                )),
                                interval: Some(Duration::from_secs(30)),
                                notify_ref: vec![NotifyRef::Direct("meow".into())],
                            },
                            SubscriptionRaw {
                                platform: Accessor::new(SourceConfig::Twitter(
                                    Accessor::new(twitter::source::ConfigParams {
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
                                platform: Accessor::new(SourceConfig::Twitter(
                                    Accessor::new(twitter::source::ConfigParams {
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
                })
            },
        );

        Config::parse_for_test(
            r#"
interval = '1min'
reporter = { notify = ["meow"], heartbeat = { type = "HttpGet", url = "https://example.com/", interval = '1min' } } 

[notify]
meow = { platform = "Telegram", id = 1234, thread_id = 123, token = "xxx" }

[[subscription.meow]]
platform = { name = "bilibili.live", user_id = 123456 }
notify = ["meow"]
            "#,
            |c| assert!(c.is_ok()),
        );

        // Notify ref key alias, "ref" or "to"
        Config::parse_for_test(
            r#"
interval = '1min'

[notify]
meow = { platform = "Telegram", id = 1234, thread_id = 123, token = "xxx" }

[[subscription.meow]]
platform = { name = "bilibili.live", user_id = 123456 }
notify = [ { ref = "meow" }, { to = "meow" } ]
            "#,
            |c| assert!(c.is_ok()),
        );

        Config::parse_for_test(
            r#"
interval = '1min'
reporter = { log = { notify = ["reporter_notify"] }, heartbeat = { type = "HttpGet", url = "https://example.com/", interval = '1min' } } 

[[subscription.meow]]
platform = { name = "bilibili.live", user_id = 123456 }
notify = []
            "#,
            |c| {
                assert!(c
                    .unwrap_err()
                    .to_string()
                    .ends_with("reference of notify not found 'reporter_notify'"))
            },
        );

        Config::parse_for_test(
            r#"
interval = '1min'

[[subscription.meow]]
platform = { name = "bilibili.live", user_id = 123456 }
notify = ["meow"]
            "#,
            |c| {
                assert!(c
                    .unwrap_err()
                    .to_string()
                    .ends_with("reference of notify not found 'meow'"))
            },
        );

        Config::parse_for_test(
            r#"
interval = '1min'

[notify]
meow = { platform = "Telegram", id = 1234, thread_id = 123, token = "xxx" }

[[subscription.meow]]
platform = { name = "bilibili.live", user_id = 123456 }
notify = ["meow", "woof"]
            "#,
            |c| {
                assert!(c
                    .unwrap_err()
                    .to_string()
                    .ends_with("reference of notify not found 'woof'"))
            },
        );

        Config::parse_for_test(
            r#"
interval = '1min'

[notify]
meow = { platform = "Telegram", id = 1234, thread_id = 123 }

[[subscription.meow]]
platform = { name = "bilibili.live", user_id = 123456 }
notify = ["meow"]
            "#,
            |c| {
                assert!(c
                    .unwrap_err()
                    .to_string()
                    .ends_with("both token in global and notify are missing"))
            },
        );
    }

    #[test]
    fn option_override() {
        Config::parse_for_test(
            r#"
interval = '1min'

[notify]
meow = { platform = "Telegram", id = 1234, thread_id = 123, token = "xxx" }
woof = { platform = "Telegram", id = 5678, thread_id = 456, token = "yyy" }

[[subscription.meow]]
platform = { name = "bilibili.live", user_id = 123456 }
notify = ["meow", { ref = "woof", thread_id = 114 }, { ref = "woof", notifications = { post = false } }]
            "#,
            |c| {
                let subscriptions = c.unwrap().subscriptions().collect::<Vec<_>>();

                assert_eq!(
                    subscriptions,
                    vec![(
                        "meow".into(),
                        SubscriptionRef {
                            platform: &Accessor::new(SourceConfig::BilibiliLive(Accessor::new(
                                bilibili::source::live::ConfigParams { user_id: 123456 }
                            ))),
                            interval: None,
                            notify: vec![
                                Accessor::new(NotifierConfig::Telegram(Accessor::new(
                                    telegram::notify::ConfigParams {
                                        notifications: Notifications::default(),
                                        chat: telegram::ConfigChat::Id(1234),
                                        thread_id: Some(123),
                                        token: Some(telegram::ConfigToken::with_raw("xxx")),
                                    }
                                ))),
                                Accessor::new(NotifierConfig::Telegram(Accessor::new(
                                    telegram::notify::ConfigParams {
                                        notifications: Notifications::default(),
                                        chat: telegram::ConfigChat::Id(5678),
                                        thread_id: Some(114),
                                        token: Some(telegram::ConfigToken::with_raw("yyy")),
                                    }
                                ))),
                                Accessor::new(NotifierConfig::Telegram(Accessor::new(
                                    telegram::notify::ConfigParams {
                                        notifications: Notifications {
                                            post: false,
                                            ..Default::default()
                                        },
                                        chat: telegram::ConfigChat::Id(5678),
                                        thread_id: Some(456),
                                        token: Some(telegram::ConfigToken::with_raw("yyy")),
                                    }
                                )))
                            ],
                        }
                    ),]
                );
            },
        );
    }
}
