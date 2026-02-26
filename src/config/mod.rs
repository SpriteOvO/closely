mod accounts;
mod overridable;
mod secret;
mod validator;

use std::{collections::HashMap, time::Duration};

pub use accounts::*;
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
    source::SourceConfig,
};

#[derive(Debug, PartialEq, Deserialize)]
pub struct Config {
    #[serde(with = "humantime_serde")]
    pub interval: Duration,
    // Distribute intervals equidistantly, which helps avoid periodic CPU peaks and request peaks
    // when there are many tasks.
    #[serde(default)]
    pub equidistant_intervals: bool,
    reporter: Accessor<Option<ConfigReporterRaw>>,
    #[serde(default)]
    platform: Accessor<PlatformGlobal>,
    #[serde(rename = "notify", default)]
    notify_map: Accessor<NotifyMap>,
    subscription: HashMap<String, Vec<SubscriptionRaw>>,
}

#[cfg(not(test))]
static CONFIG: std::sync::OnceLock<Config> = std::sync::OnceLock::new();
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
                        interval: subscription
                            .interval
                            .or(self.platform.global_interval_of(&subscription.platform))
                            .unwrap_or(self.interval),
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

impl PlatformGlobal {
    pub fn global_interval_of(&self, source: &Accessor<SourceConfig>) -> Option<Duration> {
        match &**source {
            SourceConfig::BilibiliSpace(_) => self
                .bilibili
                .as_ref()
                .and_then(|b| b.space.as_ref().and_then(|p| p.interval)),
            SourceConfig::BilibiliLive(_)
            | SourceConfig::BilibiliVideo(_)
            | SourceConfig::BilibiliPlayback(_)
            | SourceConfig::TwitterPost(_)
            | SourceConfig::TwitterReply(_)
            | SourceConfig::GitHubIssuePr(_)
            | SourceConfig::Rss(_) => None,
        }
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
    pub interval: Duration,
    pub notify: Vec<Accessor<NotifierConfig>>,
}

// Should be always used with `#[serde(flatten)]`
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct NotifierBase<O = ()> {
    #[serde(default = "helper::refl_bool::<true>")]
    pub enable: bool,
    #[serde(default)]
    pub switch: NotificationSwitch,
    #[serde(default)]
    pub option: NotificationOption<O>,
}

impl<'a, O: Default + Deserialize<'a>> Default for NotifierBase<O> {
    fn default() -> Self {
        helper::serde_default()
    }
}

impl<'a, O: Deserialize<'a> + Overridable> Overridable for NotifierBase<O> {
    type Override = NotifierBaseOverride<O::Override>;

    fn override_into(self, new: Self::Override) -> Self
    where
        Self: Sized,
    {
        Self {
            enable: new.enable.unwrap_or(self.enable),
            switch: match new.switch {
                Some(switch) => self.switch.override_into(switch),
                None => self.switch,
            },
            option: match new.option {
                Some(option) => self.option.override_into(option),
                None => self.option,
            },
        }
    }
}

// Should be always used with `#[serde(flatten)]`
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct NotifierBaseOverride<O = ()> {
    enable: Option<bool>,
    switch: Option<NotificationSwitchOverride>,
    option: Option<NotificationOptionOverride<O>>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct NotificationSwitch {
    #[serde(default = "helper::refl_bool::<true>")]
    pub live_online: bool,
    #[serde(default = "helper::refl_bool::<false>")]
    pub live_title: bool,
    #[serde(default = "helper::refl_bool::<true>")]
    pub post: bool,
    #[serde(default = "helper::refl_bool::<true>")]
    pub article: bool,
    #[serde(default = "helper::refl_bool::<true>")]
    pub feed: bool,
    #[serde(default = "helper::refl_bool::<true>")]
    pub log: bool,
    #[serde(default = "helper::refl_bool::<true>")]
    pub playback: bool,
    #[serde(default = "helper::refl_bool::<true>")]
    pub document: bool,
}

impl Default for NotificationSwitch {
    fn default() -> Self {
        helper::serde_default()
    }
}

impl Overridable for NotificationSwitch {
    type Override = NotificationSwitchOverride;

    fn override_into(self, new: Self::Override) -> Self
    where
        Self: Sized,
    {
        Self {
            live_online: new.live_online.unwrap_or(self.live_online),
            live_title: new.live_title.unwrap_or(self.live_title),
            post: new.post.unwrap_or(self.post),
            article: new.article.unwrap_or(self.article),
            feed: new.feed.unwrap_or(self.feed),
            log: new.log.unwrap_or(self.log),
            playback: new.playback.unwrap_or(self.playback),
            document: new.document.unwrap_or(self.document),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct NotificationSwitchOverride {
    pub live_online: Option<bool>,
    pub live_title: Option<bool>,
    pub post: Option<bool>,
    pub article: Option<bool>,
    pub feed: Option<bool>,
    pub log: Option<bool>,
    pub playback: Option<bool>,
    pub document: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct NotificationOption<O> {
    #[serde(default = "helper::refl_bool::<true>")]
    pub platform_name: bool,
    #[serde(default = "helper::refl_bool::<false>")]
    pub author_name: bool,
    #[serde(default = "helper::refl_bool::<true>")]
    pub article_tag: bool,
    #[serde(default, flatten)]
    pub ext: O,
}

impl<'a, O: Default + Deserialize<'a>> Default for NotificationOption<O> {
    fn default() -> Self {
        helper::serde_default()
    }
}

impl<'a, O: Deserialize<'a> + Overridable> Overridable for NotificationOption<O> {
    type Override = NotificationOptionOverride<O::Override>;

    fn override_into(self, new: Self::Override) -> Self
    where
        Self: Sized,
    {
        Self {
            platform_name: new.platform_name.unwrap_or(self.platform_name),
            author_name: new.author_name.unwrap_or(self.author_name),
            article_tag: new.article_tag.unwrap_or(self.article_tag),
            ext: match new.ext {
                Some(ext) => self.ext.override_into(ext),
                None => self.ext,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct NotificationOptionOverride<O> {
    pub platform_name: Option<bool>,
    pub author_name: Option<bool>,
    pub article_tag: Option<bool>,
    #[serde(flatten)]
    pub ext: Option<O>,
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
    use reqwest::Url;

    use super::*;
    use crate::reporter::{
        ConfigHeartbeat, ConfigHeartbeatHttpGet, ConfigHeartbeatKind, ConfigReporterLog,
        ConfigReporterLogOpenTelemetry,
    };

    #[test]
    fn deser() {
        Config::parse_for_test(
            r#"
interval = '1min'
equidistant_intervals = true

[reporter.log]
notify = ["meow"]
opentelemetry = { endpoint = "http://localhost:4317" }

[reporter.heartbeat]
type = "HttpGet"
url = "https://example.com/"
interval = '1min'

[platform.QQ.account.MyQQ]
onebot11 = { remote_http = "http://localhost:8000/" }

[platform.Telegram]
token = "ttt"

[platform.Twitter.account.MyTwitter]
cookies = "a=b;c=d;ct0=blah"

[platform.bilibili]
space = { interval = '10m' }
playback = { bililive_recorder = { listen_webhook = { host = "127.0.0.1", port = 8888 }, working_directory = "/brec/" } }

[notify]
meow = { platform = "Telegram", id = 1234, thread_id = 123, token = "xxx" }
woof = { platform = "Telegram", id = 5678, thread_id = 900, switch = { post = false } }

[[subscription.meow]]
platform = { name = "bilibili.live", user_id = 123456 }
interval = '30s'
notify = ["meow"]

[[subscription.meow]]
platform = { name = "bilibili.space", user_id = 123456 }
notify = ["meow"]

[[subscription.meow]]
platform = { name = "Twitter", username = "meowww", as = "MyTwitter" }
notify = ["meow", "woof"]

[[subscription.meow]]
platform = { name = "Twitter", username = "meowww2", as = "MyTwitter" }
notify = ["meow", "woof", { ref = "woof", id = 123 }]
            "#,
            |c| {
                assert_eq!(c.unwrap(), &Config {
                    interval: Duration::from_secs(60), // 1min
                    equidistant_intervals: true,
                    reporter: Accessor::new(Some(ConfigReporterRaw {
                        log: Accessor::new(Some(ConfigReporterLog {
                            opentelemetry: Some(ConfigReporterLogOpenTelemetry {
                                endpoint: Url::parse("http://localhost:4317").unwrap(),
                            }),
                            notify_ref: Some(vec![NotifyRef::Direct("meow".into())]),
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
                            account: Accounts::from_iter([
                                ("MyQQ".into(), Accessor::new(qq::ConfigAccount {
                                    onebot11: qq::onebot11::ConfigOneBot11 {
                                        remote_http: Url::parse("http://localhost:8000/").unwrap(),
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
                            account: Accounts::from_iter([("MyTwitter".into(), Accessor::new(ConfigCookies::with_raw("a=b;c=d;ct0=blah")))])
                        })),
                        bilibili: Accessor::new(Some(bilibili::ConfigGlobal {
                            cookies: Accessor::new(None),
                            space: Accessor::new(Some(bilibili::source::space::ConfigGlobal {
                                interval: Some(Duration::from_secs(600))
                            })),
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
                                base: NotifierBase::default(),
                                chat: telegram::ConfigChat::Id(1234),
                                thread_id: Some(123),
                                token: Some(telegram::ConfigToken::with_raw("xxx")),
                            })))
                        ),
                        (
                            "woof".into(),
                            Accessor::new(NotifierConfig::Telegram(Accessor::new(telegram::notify::ConfigParams {
                                base: NotifierBase {
                                    enable: true,
                                    switch: NotificationSwitch {
                                        live_online: true,
                                        live_title: false,
                                        post: false,
                                        article: true,
                                        feed: true,
                                        log: true,
                                        playback: true,
                                        document: true,
                                    },
                                    option: NotificationOption {
                                        platform_name: true,
                                        author_name: false,
                                        article_tag: true,
                                        ext: telegram::notify::OptionExt {
                                            no_button: false,
                                        }
                                    }
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
                                platform: Accessor::new(SourceConfig::BilibiliSpace(
                                    Accessor::new(bilibili::source::space::ConfigParams { user_id: 123456 })
                                )),
                                interval: None,
                                notify_ref: vec![NotifyRef::Direct("meow".into())],
                            },
                            SubscriptionRaw {
                                platform: Accessor::new(SourceConfig::TwitterPost(
                                    Accessor::new(twitter::source::post::ConfigParams {
                                        username: "meowww".into(),
                                        actor: AccountRef::new("MyTwitter")
                                    })
                                )),
                                interval: None,
                                notify_ref: vec![
                                    NotifyRef::Direct("meow".into()),
                                    NotifyRef::Direct("woof".into())
                                ],
                            },
                            SubscriptionRaw {
                                platform: Accessor::new(SourceConfig::TwitterPost(
                                    Accessor::new(twitter::source::post::ConfigParams {
                                        username: "meowww2".into(),
                                        actor: AccountRef::new("MyTwitter")
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
                assert!(
                    c.unwrap_err()
                        .to_string()
                        .ends_with("reference of notify not found 'reporter_notify'")
                )
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
                assert!(
                    c.unwrap_err()
                        .to_string()
                        .ends_with("reference of notify not found 'meow'")
                )
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
                assert!(
                    c.unwrap_err()
                        .to_string()
                        .ends_with("reference of notify not found 'woof'")
                )
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
                assert!(
                    c.unwrap_err()
                        .to_string()
                        .ends_with("both token in global and notify are missing")
                )
            },
        );
    }

    #[test]
    fn option_override() {
        Config::parse_for_test(
            r#"
interval = '1min'

[platform.bilibili.space]
interval = '10m'

[notify]
meow = { platform = "Telegram", id = 1234, thread_id = 123, token = "xxx" }
woof = { platform = "Telegram", id = 5678, thread_id = 456, token = "yyy" }

[[subscription.meow]]
platform = { name = "bilibili.live", user_id = 123456 }
notify = ["meow", { ref = "woof", thread_id = 114 }, { ref = "woof", switch = { post = false } }]

[[subscription.meow]]
platform = { name = "bilibili.space", user_id = 123456 }
notify = []
            "#,
            |c| {
                let subscriptions = c.unwrap().subscriptions().collect::<Vec<_>>();

                assert_eq!(
                    subscriptions,
                    vec![
                        (
                            "meow".into(),
                            SubscriptionRef {
                                platform: &Accessor::new(SourceConfig::BilibiliLive(
                                    Accessor::new(bilibili::source::live::ConfigParams {
                                        user_id: 123456
                                    })
                                )),
                                interval: Duration::from_secs(60),
                                notify: vec![
                                    Accessor::new(NotifierConfig::Telegram(Accessor::new(
                                        telegram::notify::ConfigParams {
                                            base: NotifierBase::default(),
                                            chat: telegram::ConfigChat::Id(1234),
                                            thread_id: Some(123),
                                            token: Some(telegram::ConfigToken::with_raw("xxx")),
                                        }
                                    ))),
                                    Accessor::new(NotifierConfig::Telegram(Accessor::new(
                                        telegram::notify::ConfigParams {
                                            base: NotifierBase::default(),
                                            chat: telegram::ConfigChat::Id(5678),
                                            thread_id: Some(114),
                                            token: Some(telegram::ConfigToken::with_raw("yyy")),
                                        }
                                    ))),
                                    Accessor::new(NotifierConfig::Telegram(Accessor::new(
                                        telegram::notify::ConfigParams {
                                            base: NotifierBase {
                                                enable: true,
                                                switch: NotificationSwitch {
                                                    post: false,
                                                    ..Default::default()
                                                },
                                                option: NotificationOption::default()
                                            },
                                            chat: telegram::ConfigChat::Id(5678),
                                            thread_id: Some(456),
                                            token: Some(telegram::ConfigToken::with_raw("yyy")),
                                        }
                                    )))
                                ],
                            }
                        ),
                        (
                            "meow".into(),
                            SubscriptionRef {
                                platform: &Accessor::new(SourceConfig::BilibiliSpace(
                                    Accessor::new(bilibili::source::space::ConfigParams {
                                        user_id: 123456
                                    })
                                )),
                                interval: Duration::from_secs(600),
                                notify: vec![]
                            }
                        )
                    ]
                );
            },
        );
    }
}
