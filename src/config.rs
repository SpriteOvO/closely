use std::{borrow::Cow, collections::HashMap, env, fmt, time::Duration};

use anyhow::{anyhow, bail, ensure};
use once_cell::sync::OnceCell;
use serde::{
    de::{
        value::{Error as SerdeValueError, MapDeserializer},
        DeserializeOwned,
    },
    Deserialize,
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
    pub telegram: Option<PlatformGlobalTelegram>,
    #[serde(rename = "Twitter")]
    pub twitter: Option<PlatformGlobalTwitter>,
}

impl PlatformGlobal {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(telegram) = &self.telegram {
            telegram.token.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PlatformGlobalTelegram {
    #[serde(flatten)]
    pub token: TelegramToken,
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

    fn validate(&self, global: &PlatformGlobal) -> anyhow::Result<()> {
        self.0
            .values()
            .try_for_each(|notify| notify.validate(global))
    }
}

trait Overridable {
    type Override: DeserializeOwned;

    fn override_into(self, new: Self::Override) -> Self
    where
        Self: Sized;
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "platform")]
pub enum Notify {
    Telegram(NotifyTelegram),
}

impl Notify {
    fn validate(&self, global: &PlatformGlobal) -> anyhow::Result<()> {
        match self {
            Notify::Telegram(v) => match &v.token {
                Some(token) => token.validate().map_err(|err| anyhow!("[Telegram] {err}")),
                None => {
                    ensure!(
                        global.telegram.is_some(),
                        "[Telegram] both token in global and notify are missing"
                    );
                    Ok(())
                }
            },
        }
    }

    fn override_into(self, new: toml::Value) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        match self {
            Notify::Telegram(n) => {
                let new: <NotifyTelegram as Overridable>::Override = new.try_into()?;
                Ok(Notify::Telegram(n.override_into(new)))
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
    #[serde(default)]
    pub notifications: Notifications,
    #[serde(flatten)]
    pub chat: TelegramChat,
    pub thread_id: Option<i64>,
    #[serde(flatten)]
    pub token: Option<TelegramToken>,
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

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotifyTelegramOverride {
    pub notifications: Option<NotificationsOverride>,
    #[serde(flatten)]
    pub chat: Option<TelegramChat>,
    pub thread_id: Option<i64>,
    #[serde(flatten)]
    token: Option<TelegramToken>,
}

impl Overridable for NotifyTelegram {
    type Override = NotifyTelegramOverride;

    fn override_into(self, new: Self::Override) -> Self
    where
        Self: Sized,
    {
        Self {
            notifications: match new.notifications {
                Some(notifications) => self.notifications.override_into(notifications),
                None => self.notifications,
            },
            chat: new.chat.unwrap_or(self.chat),
            thread_id: new.thread_id.or(self.thread_id),
            token: new.token.or(self.token),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelegramChat {
    Id(i64),
    Username(String),
}

impl fmt::Display for TelegramChat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TelegramChat::Id(id) => write!(f, "{}", id),
            TelegramChat::Username(username) => write!(f, "@{}", username),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelegramToken {
    Token(String),
    TokenEnv(String),
}

impl TelegramToken {
    pub fn get(&self) -> anyhow::Result<Cow<str>> {
        match &self {
            Self::Token(token) => Ok(Cow::Borrowed(token)),
            Self::TokenEnv(token_env) => Ok(Cow::Owned(env::var(token_env)?)),
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        match &self {
            Self::Token(_) => Ok(()),
            Self::TokenEnv(token_env) => match env::var(token_env) {
                Ok(_) => Ok(()),
                Err(err) => bail!("{err} ({token_env})"),
            },
        }
    }
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
                    telegram: Some(PlatformGlobalTelegram {
                        token: TelegramToken::Token("ttt".into())
                    }),
                    twitter: Some(PlatformGlobalTwitter {
                        nitter_host: "https://nitter.example.com/".into()
                    })
                }),
                notify_map: NotifyMap(HashMap::from_iter([
                    (
                        "meow".into(),
                        Notify::Telegram(NotifyTelegram {
                            notifications: Notifications::default(),
                            chat: TelegramChat::Id(1234),
                            thread_id: Some(123),
                            token: Some(TelegramToken::Token("xxx".into())),
                        })
                    ),
                    (
                        "woof".into(),
                        Notify::Telegram(NotifyTelegram {
                            notifications: Notifications {
                                live_online: true,
                                live_title: false,
                                post: false,
                            },
                            chat: TelegramChat::Id(5678),
                            thread_id: Some(900),
                            token: None,
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

        assert!(Config::from_str(
            r#"
interval = '1min'

[notify]
meow = { platform = "Telegram", id = 1234, thread_id = 123 }

[[subscription.meow]]
platform = { name = "bilibili.live", uid = 123456 }
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
platform = { name = "bilibili.live", uid = 123456 }
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
                    platform: &SourcePlatform::BilibiliLive(SourcePlatformBilibiliLive {
                        uid: 123456
                    }),
                    interval: None,
                    notify: vec![
                        Notify::Telegram(NotifyTelegram {
                            notifications: Notifications::default(),
                            chat: TelegramChat::Id(1234),
                            thread_id: Some(123),
                            token: Some(TelegramToken::Token("xxx".into())),
                        }),
                        Notify::Telegram(NotifyTelegram {
                            notifications: Notifications::default(),
                            chat: TelegramChat::Id(5678),
                            thread_id: Some(114),
                            token: Some(TelegramToken::Token("yyy".into())),
                        }),
                        Notify::Telegram(NotifyTelegram {
                            notifications: Notifications {
                                post: false,
                                ..Default::default()
                            },
                            chat: TelegramChat::Id(5678),
                            thread_id: Some(456),
                            token: Some(TelegramToken::Token("yyy".into())),
                        })
                    ],
                }
            ),]
        );
    }
}
