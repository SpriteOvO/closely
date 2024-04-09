use std::{borrow::Cow, collections::HashMap, env, time::Duration};

use anyhow::{anyhow, bail};
use once_cell::sync::OnceCell;
use serde::{
    de::{
        value::{Error as SerdeValueError, MapDeserializer},
        DeserializeOwned,
    },
    Deserialize,
};

use crate::{
    notify,
    source::{self, ConfigSourcePlatform},
};

#[derive(Debug, PartialEq, Deserialize)]
pub struct Config {
    #[serde(with = "humantime_serde")]
    pub interval: Duration,
    platform: Option<PlatformGlobal>,
    #[serde(rename = "notify", default)]
    notify_map: NotifyMap,
    subscription: HashMap<String, Vec<SubscriptionRaw>>,
}

static PLATFORM_GLOBAL: OnceCell<PlatformGlobal> = OnceCell::new();

impl Config {
    pub fn init(input: impl AsRef<str>) -> anyhow::Result<Self> {
        let mut config = Self::from_str(input)?;
        PLATFORM_GLOBAL
            .set(config.platform.take().unwrap_or_default())
            .map_err(|_| anyhow!("config was initialized before"))?;
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
        // Validate platform_global
        if let Some(platform) = &self.platform {
            platform.validate()?;
        }

        // Validate notify ref
        self.subscription
            .values()
            .flatten()
            .flat_map(|subscription| &subscription.notify_ref)
            .map(|notify_ref| self.notify_map.get_by_ref(notify_ref))
            .collect::<Result<Vec<_>, _>>()?;

        // Validate notify_map
        self.notify_map
            .validate(self.platform.as_ref().unwrap_or(&PlatformGlobal::default()))?;

        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Default, Deserialize)]
pub struct PlatformGlobal {
    #[serde(rename = "Telegram")]
    pub telegram: Option<notify::telegram::ConfigGlobal>,
    #[serde(rename = "Twitter")]
    pub twitter: Option<source::twitter::ConfigGlobal>,
}

impl PlatformGlobal {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(telegram) = &self.telegram {
            telegram.token.as_secret_ref().validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct SubscriptionRaw {
    pub platform: ConfigSourcePlatform,
    #[serde(default, with = "humantime_serde")]
    pub interval: Option<Duration>,
    #[serde(rename = "notify")]
    notify_ref: Vec<NotifyRef>,
}

#[derive(Debug, PartialEq)]
pub struct SubscriptionRef<'a> {
    pub platform: &'a ConfigSourcePlatform,
    pub interval: Option<Duration>,
    pub notify: Vec<notify::ConfigNotify>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Notifications {
    #[serde(default = "default_true")]
    pub live_online: bool,
    #[serde(default = "default_false")]
    pub live_title: bool,
    #[serde(default = "default_true")]
    pub post: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        // https://stackoverflow.com/a/77858562
        Self::deserialize(MapDeserializer::<_, SerdeValueError>::new(
            std::iter::empty::<((), ())>(),
        ))
        .unwrap()
    }
}

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
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct NotificationsOverride {
    pub live_online: Option<bool>,
    pub live_title: Option<bool>,
    pub post: Option<bool>,
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
pub struct NotifyMap(#[serde(default)] HashMap<String, notify::ConfigNotify>);

impl NotifyMap {
    fn get_by_ref(&self, notify_ref: &NotifyRef) -> anyhow::Result<notify::ConfigNotify> {
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

pub trait AsSecretRef {
    fn as_secret_ref(&self) -> SecretRef;
}

pub enum SecretRef<'a> {
    Lit(&'a str),
    Env(&'a str),
}

impl<'a> SecretRef<'a> {
    pub fn get(&self) -> anyhow::Result<Cow<'a, str>> {
        match self {
            Self::Lit(lit) => Ok(Cow::Borrowed(lit)),
            Self::Env(key) => Ok(Cow::Owned(env::var(key)?)),
        }
    }

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

pub trait Overridable {
    type Override: DeserializeOwned;

    fn override_into(self, new: Self::Override) -> Self
    where
        Self: Sized;
}

const fn default_true() -> bool {
    true
}

const fn default_false() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deser() {
        assert_eq!(
            Config::from_str(
                r#"
interval = '1min'

[platform."Telegram"]
token = "ttt"

[platform."Twitter"]
nitter_host = "https://nitter.example.com/"

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
                platform: Some(PlatformGlobal {
                    telegram: Some(notify::telegram::ConfigGlobal {
                        token: notify::telegram::ConfigToken::Token("ttt".into())
                    }),
                    twitter: Some(source::twitter::ConfigGlobal {
                        nitter_host: "https://nitter.example.com/".into()
                    })
                }),
                notify_map: NotifyMap(HashMap::from_iter([
                    (
                        "meow".into(),
                        notify::ConfigNotify::Telegram(notify::telegram::ConfigParams {
                            notifications: Notifications::default(),
                            chat: notify::telegram::ConfigChat::Id(1234),
                            thread_id: Some(123),
                            token: Some(notify::telegram::ConfigToken::Token("xxx".into())),
                        })
                    ),
                    (
                        "woof".into(),
                        notify::ConfigNotify::Telegram(notify::telegram::ConfigParams {
                            notifications: Notifications {
                                live_online: true,
                                live_title: false,
                                post: false,
                            },
                            chat: notify::telegram::ConfigChat::Id(5678),
                            thread_id: Some(900),
                            token: None,
                        })
                    )
                ])),
                subscription: HashMap::from_iter([(
                    "meow".into(),
                    vec![
                        SubscriptionRaw {
                            platform: ConfigSourcePlatform::BilibiliLive(
                                source::bilibili::live::ConfigParams { user_id: 123456 }
                            ),
                            interval: Some(Duration::from_secs(30)),
                            notify_ref: vec![NotifyRef::Direct("meow".into())],
                        },
                        SubscriptionRaw {
                            platform: ConfigSourcePlatform::Twitter(
                                source::twitter::ConfigParams {
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
                            platform: ConfigSourcePlatform::Twitter(
                                source::twitter::ConfigParams {
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
                    platform: &ConfigSourcePlatform::BilibiliLive(
                        source::bilibili::live::ConfigParams { user_id: 123456 }
                    ),
                    interval: None,
                    notify: vec![
                        notify::ConfigNotify::Telegram(notify::telegram::ConfigParams {
                            notifications: Notifications::default(),
                            chat: notify::telegram::ConfigChat::Id(1234),
                            thread_id: Some(123),
                            token: Some(notify::telegram::ConfigToken::Token("xxx".into())),
                        }),
                        notify::ConfigNotify::Telegram(notify::telegram::ConfigParams {
                            notifications: Notifications::default(),
                            chat: notify::telegram::ConfigChat::Id(5678),
                            thread_id: Some(114),
                            token: Some(notify::telegram::ConfigToken::Token("yyy".into())),
                        }),
                        notify::ConfigNotify::Telegram(notify::telegram::ConfigParams {
                            notifications: Notifications {
                                post: false,
                                ..Default::default()
                            },
                            chat: notify::telegram::ConfigChat::Id(5678),
                            thread_id: Some(456),
                            token: Some(notify::telegram::ConfigToken::Token("yyy".into())),
                        })
                    ],
                }
            ),]
        );
    }
}
