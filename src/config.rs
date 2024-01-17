use std::{borrow::Cow, collections::HashMap, env, fmt, sync::Arc, time::Duration};

use anyhow::anyhow;
use serde::Deserialize;

#[derive(Debug, PartialEq, Deserialize)]
pub struct Config {
    #[serde(with = "humantime_serde")]
    pub interval: Duration,
    #[serde(default)]
    notify: HashMap<String, Arc<Notify>>,
    subscription: HashMap<String, Vec<Subscription>>,
}

impl Config {
    pub fn from_str(input: impl AsRef<str>) -> anyhow::Result<Self> {
        let config = toml::from_str::<Self>(input.as_ref())?;
        config.validate()?;
        Ok(config)
    }

    pub fn subscriptions(
        &self,
    ) -> impl Iterator<Item = (String, (Arc<Notify>, Arc<Platform>))> + '_ {
        self.subscription.iter().flat_map(|(name, subscriptions)| {
            subscriptions.iter().map(|subscription| {
                (
                    name.clone(),
                    (
                        Arc::clone(self.notify.get(&subscription.notify).unwrap()),
                        Arc::clone(&subscription.platform),
                    ),
                )
            })
        })
    }
}

impl Config {
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

#[derive(Debug, PartialEq, Deserialize)]
pub struct Subscription {
    #[serde(flatten)]
    pub platform: Arc<Platform>,
    notify: String,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(tag = "platform")]
pub enum Platform {
    #[serde(rename = "live.bilibili.com")]
    LiveBilibiliCom(PlatformLiveBilibiliCom),
    #[serde(rename = "twitter.com")]
    TwitterCom(PlatformTwitterCom),
    // Yea! PRs for supports of more platforms are welcome!
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Platform::LiveBilibiliCom(p) => write!(f, "live.bilibili.com:{}", p.uid),
            Platform::TwitterCom(p) => write!(f, "twitter.com:{}", p.username),
        }
    }
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct PlatformLiveBilibiliCom {
    pub uid: u64,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct PlatformTwitterCom {
    pub username: String,
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

[notify.meow]
telegram = [ { id = 1234, thread_id = 123, token = "xxx" } ]

[[subscription.meow]]
platform = "live.bilibili.com"
uid = 123456
notify = "meow"
                "#,
            )
            .unwrap(),
            Config {
                interval: Duration::from_secs(60), // 1min
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
                    vec![Subscription {
                        platform: Arc::new(Platform::LiveBilibiliCom(PlatformLiveBilibiliCom {
                            uid: 123456
                        })),
                        notify: "meow".into(),
                    }]
                )]),
            }
        );

        assert_eq!(
            Config::from_str(
                r#"
interval = '1min'

[[subscription.meow]]
platform = "live.bilibili.com"
uid = 123456
notify = "meow"
                "#
            )
            .unwrap_err()
            .to_string(),
            "reference of notify not found"
        )
    }
}
