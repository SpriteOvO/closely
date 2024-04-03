use std::{borrow::Cow, collections::HashMap, env, fmt, time::Duration};

use anyhow::{anyhow, bail};
use once_cell::sync::OnceCell;
use serde::{de::DeserializeOwned, Deserialize};

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
        // Validate notify ref
        self.subscription
            .values()
            .flatten()
            .flat_map(|subscription| &subscription.notify_ref)
            .map(|notify_ref| self.notify_map.get_by_ref(notify_ref))
            .collect::<Result<Vec<_>, _>>()?;

        // Validate notify_map
        self.notify_map.validate()?;

        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Default, Deserialize)]
pub struct PlatformGlobal {
    #[serde(rename = "Twitter")]
    pub twitter: Option<PlatformGlobalTwitter>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PlatformGlobalTwitter {
    pub nitter_host: String,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct SubscriptionRaw {
    pub platform: SourcePlatform,
    #[serde(default, with = "humantime_serde")]
    pub interval: Option<Duration>,
    #[serde(rename = "notify")]
    notify_ref: Vec<NotifyRef>,
}

#[derive(Debug, PartialEq)]
pub struct SubscriptionRef<'a> {
    pub platform: &'a SourcePlatform,
    pub interval: Option<Duration>,
    pub notify: Vec<Notify>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "name")]
#[allow(clippy::enum_variant_names)]
pub enum SourcePlatform {
    #[serde(rename = "bilibili.live")]
    BilibiliLive(SourcePlatformBilibiliLive),
    #[serde(rename = "bilibili.space")]
    BilibiliSpace(SourcePlatformBilibiliSpace),
    #[serde(rename = "Twitter")]
    Twitter(SourcePlatformTwitter),
    // Yea! PRs for supports of more platforms are welcome!
}

impl fmt::Display for SourcePlatform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourcePlatform::BilibiliLive(p) => write!(f, "{p}"),
            SourcePlatform::BilibiliSpace(p) => write!(f, "{p}"),
            SourcePlatform::Twitter(p) => write!(f, "{p}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SourcePlatformBilibiliLive {
    pub uid: u64,
}

impl fmt::Display for SourcePlatformBilibiliLive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bilibili.live:{}", self.uid)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SourcePlatformBilibiliSpace {
    pub uid: u64,
}

impl fmt::Display for SourcePlatformBilibiliSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "space.bilibili.com:{}", self.uid)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SourcePlatformTwitter {
    pub username: String,
}

impl fmt::Display for SourcePlatformTwitter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Twitter:{}", self.username)
    }
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
pub struct NotifyMap(#[serde(default)] HashMap<String, Notify>);

impl NotifyMap {
    fn get_by_ref(&self, notify_ref: &NotifyRef) -> anyhow::Result<Notify> {
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

    fn validate(&self) -> anyhow::Result<()> {
        self.0.values().try_for_each(|notify| notify.validate())
    }
}

trait Overridable {
    type Override: DeserializeOwned;

    fn override_into(self, new: Self::Override) -> anyhow::Result<Self>
    where
        Self: Sized;
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "platform")]
pub enum Notify {
    Telegram(NotifyTelegram),
}

impl Notify {
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Notify::Telegram(v) => v.validate().map_err(|err| anyhow!("[Telegram] {err}")),
        }
    }

    fn override_into(self, new: toml::Value) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        match self {
            Notify::Telegram(n) => {
                let new: <NotifyTelegram as Overridable>::Override = new.try_into()?;
                Ok(Notify::Telegram(n.override_into(new)?))
            }
        }
    }
}

impl fmt::Display for Notify {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Notify::Telegram(notify_telegram) => write!(f, "{}", notify_telegram),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct NotifyTelegram {
    #[serde(flatten)]
    pub chat: NotifyTelegramChat,
    pub thread_id: Option<i64>,
    #[serde(flatten)]
    token: NotifyTelegramToken,
}

impl fmt::Display for NotifyTelegram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "telegram:{}", self.chat)?;
        if let Some(thread_id) = self.thread_id {
            write!(f, ":({})", thread_id)?;
        }
        Ok(())
    }
}

impl NotifyTelegram {
    pub fn token(&self) -> anyhow::Result<Cow<str>> {
        match &self.token {
            NotifyTelegramToken::Token(token) => Ok(Cow::Borrowed(token)),
            NotifyTelegramToken::TokenEnv(token_env) => Ok(Cow::Owned(env::var(token_env)?)),
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        match &self.token {
            NotifyTelegramToken::Token(_) => Ok(()),
            NotifyTelegramToken::TokenEnv(token_env) => match env::var(token_env) {
                Ok(_) => Ok(()),
                Err(err) => bail!("{err} ({token_env})"),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotifyTelegramOverride {
    #[serde(flatten)]
    pub chat: Option<NotifyTelegramChat>,
    pub thread_id: Option<i64>,
    #[serde(flatten)]
    token: Option<NotifyTelegramToken>,
}

impl Overridable for NotifyTelegram {
    type Override = NotifyTelegramOverride;

    fn override_into(self, new: Self::Override) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        Ok(Self {
            chat: new.chat.unwrap_or(self.chat),
            thread_id: new.thread_id.or(self.thread_id),
            token: new.token.unwrap_or(self.token),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyTelegramChat {
    Id(i64),
    Username(String),
}

impl fmt::Display for NotifyTelegramChat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NotifyTelegramChat::Id(id) => write!(f, "{}", id),
            NotifyTelegramChat::Username(username) => write!(f, "@{}", username),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum NotifyTelegramToken {
    Token(String),
    TokenEnv(String),
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

[platform."Twitter"]
nitter_host = "https://nitter.example.com/"

[notify]
meow = { platform = "Telegram", id = 1234, thread_id = 123, token = "xxx" }
woof = { platform = "Telegram", id = 5678, thread_id = 900, token = "yyy" }

[[subscription.meow]]
platform = { name = "bilibili.live", uid = 123456 }
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
                    twitter: Some(PlatformGlobalTwitter {
                        nitter_host: "https://nitter.example.com/".into()
                    })
                }),
                notify_map: NotifyMap(HashMap::from_iter([
                    (
                        "meow".into(),
                        Notify::Telegram(NotifyTelegram {
                            chat: NotifyTelegramChat::Id(1234),
                            thread_id: Some(123),
                            token: NotifyTelegramToken::Token("xxx".into()),
                        })
                    ),
                    (
                        "woof".into(),
                        Notify::Telegram(NotifyTelegram {
                            chat: NotifyTelegramChat::Id(5678),
                            thread_id: Some(900),
                            token: NotifyTelegramToken::Token("yyy".into()),
                        })
                    )
                ])),
                subscription: HashMap::from_iter([(
                    "meow".into(),
                    vec![
                        SubscriptionRaw {
                            platform: SourcePlatform::BilibiliLive(SourcePlatformBilibiliLive {
                                uid: 123456
                            }),
                            interval: Some(Duration::from_secs(30)),
                            notify_ref: vec![NotifyRef::Direct("meow".into())],
                        },
                        SubscriptionRaw {
                            platform: SourcePlatform::Twitter(SourcePlatformTwitter {
                                username: "meowww".into()
                            }),
                            interval: None,
                            notify_ref: vec![
                                NotifyRef::Direct("meow".into()),
                                NotifyRef::Direct("woof".into())
                            ],
                        },
                        SubscriptionRaw {
                            platform: SourcePlatform::Twitter(SourcePlatformTwitter {
                                username: "meowww2".into()
                            }),
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
platform = { name = "bilibili.live", uid = 123456 }
notify = ["meow"]
                "#
        )
        .is_ok());

        assert!(Config::from_str(
            r#"
interval = '1min'

[[subscription.meow]]
platform = { name = "bilibili.live", uid = 123456 }
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
platform = { name = "bilibili.live", uid = 123456 }
notify = ["meow", "woof"]
                "#
        )
        .unwrap_err()
        .to_string()
        .ends_with("reference of notify not found 'woof'"));
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
platform = { name = "bilibili.live", uid = 123456 }
notify = ["meow", { ref = "woof", thread_id = 114 }]
                "#,
        )
        .unwrap();

        let subscriptions = config.subscriptions().collect::<Vec<_>>();

        assert_eq!(
            subscriptions,
            vec![(
                "meow".into(),
                SubscriptionRef {
                    platform: &SourcePlatform::BilibiliLive(SourcePlatformBilibiliLive {
                        uid: 123456
                    }),
                    interval: None,
                    notify: vec![
                        Notify::Telegram(NotifyTelegram {
                            chat: NotifyTelegramChat::Id(1234),
                            thread_id: Some(123),
                            token: NotifyTelegramToken::Token("xxx".into()),
                        }),
                        Notify::Telegram(NotifyTelegram {
                            chat: NotifyTelegramChat::Id(5678),
                            thread_id: Some(114),
                            token: NotifyTelegramToken::Token("yyy".into()),
                        })
                    ],
                }
            ),]
        );
    }
}
