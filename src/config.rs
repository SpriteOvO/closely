use std::{borrow::Cow, collections::HashMap, env, fmt, sync::Arc, time::Duration};

use anyhow::anyhow;
use once_cell::sync::OnceCell;
use serde::Deserialize;

#[derive(Debug, PartialEq, Deserialize)]
pub struct Config {
    #[serde(with = "humantime_serde")]
    pub interval: Duration,
    platform: Option<PlatformGlobal>,
    #[serde(default)]
    notify: HashMap<String, Arc<Notify>>,
    subscription: HashMap<String, Vec<Subscription>>,
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

    pub fn subscriptions(&self) -> impl Iterator<Item = (String, (Arc<Notify>, Platform))> + '_ {
        self.subscription.iter().flat_map(|(name, subscriptions)| {
            subscriptions.iter().map(|subscription| {
                (
                    name.clone(),
                    (
                        Arc::clone(self.notify.get(&subscription.notify).unwrap()),
                        subscription.platform.clone(),
                    ),
                )
            })
        })
    }
}

impl Config {
    fn from_str(input: impl AsRef<str>) -> anyhow::Result<Self> {
        let config = toml::from_str::<Self>(input.as_ref())?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        self.subscription
            .values()
            .flatten()
            .map(|subscription| &subscription.notify)
            .all(|notify_ref| self.notify.get(notify_ref).is_some())
            .then_some(())
            .ok_or_else(|| anyhow!("reference of notify not found"))
    }
}

#[derive(Clone, Debug, PartialEq, Default, Deserialize)]
pub struct PlatformGlobal {
    #[serde(rename = "twitter.com")]
    pub twitter_com: Option<PlatformGlobalTwitterCom>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PlatformGlobalTwitterCom {
    pub nitter_host: String,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct Subscription {
    pub platform: Platform,
    notify: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "url")]
#[allow(clippy::enum_variant_names)]
pub enum Platform {
    #[serde(rename = "live.bilibili.com")]
    LiveBilibiliCom(PlatformLiveBilibiliCom),
    #[serde(rename = "space.bilibili.com")]
    SpaceBilibiliCom(PlatformSpaceBilibiliCom),
    #[serde(rename = "twitter.com")]
    TwitterCom(PlatformTwitterCom),
    // Yea! PRs for supports of more platforms are welcome!
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Platform::LiveBilibiliCom(p) => write!(f, "{p}"),
            Platform::SpaceBilibiliCom(p) => write!(f, "{p}"),
            Platform::TwitterCom(p) => write!(f, "{p}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PlatformLiveBilibiliCom {
    pub uid: u64,
}

impl fmt::Display for PlatformLiveBilibiliCom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "live.bilibili.com:{}", self.uid)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PlatformSpaceBilibiliCom {
    pub uid: u64,
}

impl fmt::Display for PlatformSpaceBilibiliCom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "space.bilibili.com:{}", self.uid)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PlatformTwitterCom {
    pub username: String,
}

impl fmt::Display for PlatformTwitterCom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "twitter.com:{}", self.username)
    }
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notify {
    Telegram(Vec<NotifyTelegram>),
}

impl fmt::Display for Notify {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Notify::Telegram(notify_telegram) => {
                for (i, notify_telegram) in notify_telegram.iter().enumerate() {
                    if i != 0 {
                        write!(f, ",")?;
                    }
                    write!(f, "{}", notify_telegram)?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, PartialEq, Deserialize)]
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
}

#[derive(Debug, PartialEq, Deserialize)]
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

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum NotifyTelegramToken {
    Token(String),
    TokenEnv(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deser_config() {
        assert_eq!(
            Config::from_str(
                r#"
interval = '1min'

[platform."twitter.com"]
nitter_host = "https://nitter.example.com/"

[notify.meow]
telegram = [ { id = 1234, thread_id = 123, token = "xxx" } ]

[[subscription.meow]]
platform = { url = "live.bilibili.com", uid = 123456 }
notify = "meow"

[[subscription.meow]]
platform = { url = "twitter.com", username = "meowww" }
notify = "meow"
                "#,
            )
            .unwrap(),
            Config {
                interval: Duration::from_secs(60), // 1min
                platform: Some(PlatformGlobal {
                    twitter_com: Some(PlatformGlobalTwitterCom {
                        nitter_host: "https://nitter.example.com/".into()
                    })
                }),
                notify: HashMap::from_iter([(
                    "meow".into(),
                    Arc::new(Notify::Telegram(vec![NotifyTelegram {
                        chat: NotifyTelegramChat::Id(1234),
                        thread_id: Some(123),
                        token: NotifyTelegramToken::Token("xxx".into()),
                    }]))
                )]),
                subscription: HashMap::from_iter([(
                    "meow".into(),
                    vec![
                        Subscription {
                            platform: Platform::LiveBilibiliCom(PlatformLiveBilibiliCom {
                                uid: 123456
                            }),
                            notify: "meow".into(),
                        },
                        Subscription {
                            platform: Platform::TwitterCom(PlatformTwitterCom {
                                username: "meowww".into()
                            }),
                            notify: "meow".into(),
                        }
                    ]
                )]),
            }
        );

        assert_eq!(
            Config::from_str(
                r#"
interval = '1min'

[[subscription.meow]]
platform = { url = "live.bilibili.com", uid = 123456 }
notify = "meow"
                "#
            )
            .unwrap_err()
            .to_string(),
            "reference of notify not found"
        )
    }
}
