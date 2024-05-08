pub mod lagrange;

use std::{
    fmt::{self, Write},
    future::Future,
    pin::Pin,
    time::Duration,
};

use anyhow::{anyhow, bail};
use once_cell::sync::Lazy;
use serde::Deserialize;
use spdlog::prelude::*;
use tokio::sync::OnceCell;

use super::NotifierTrait;
use crate::{
    config::{self, AsSecretRef},
    platform::{PlatformMetadata, PlatformTrait},
    secret_enum,
    source::{
        LiveStatus, Notification, NotificationKind, Post, PostAttachment, PostsRef, RepostFrom,
        StatusSource,
    },
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigGlobal {
    pub login: ConfigLogin,
    pub lagrange: lagrange::ConfigLagrange,
}

impl ConfigGlobal {
    pub async fn init(&self) -> anyhow::Result<()> {
        let core = lagrange::LograngeOnebot::launch(&self.login, &self.lagrange)
            .await
            .map_err(|err| anyhow!("failed to launch core: {err}"))?;

        let version = core
            .version_info_retry_timeout(Duration::from_secs(5))
            .await?;
        info!("lagrange core launched. version: {version:?}");

        BACKEND
            .set(core)
            .map_err(|_| anyhow!("global state already initialized"))
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigParams {
    #[serde(default)]
    pub notifications: config::Notifications,
    #[serde(flatten)]
    pub chat: ConfigChat,
}

impl ConfigParams {
    pub fn validate(&self, global: &config::PlatformGlobal) -> anyhow::Result<()> {
        match &global.qq {
            Some(global_qq) => global_qq.login.validate(),
            None => bail!("login in global is missing"),
        }
    }
}

impl fmt::Display for ConfigParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "QQ:{}", self.chat)?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigOverride {
    pub notifications: Option<config::NotificationsOverride>,
    #[serde(flatten)]
    pub chat: Option<ConfigChat>,
}

impl config::Overridable for ConfigParams {
    type Override = ConfigOverride;

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
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigLogin {
    #[serde(flatten)]
    pub account: ConfigAccount,
    #[serde(flatten)]
    pub password: ConfigPassword,
}

impl ConfigLogin {
    pub fn validate(&self) -> anyhow::Result<()> {
        self.account.as_secret_ref().validate()?;
        self.password.as_secret_ref().validate()?;
        Ok(())
    }
}

secret_enum! {
    #[derive(Clone, Debug, PartialEq, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ConfigAccount {
        Account(u64),
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ConfigPassword {
        Password(String),
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigChat {
    GroupId(u64),
    UserId(u64),
}

impl fmt::Display for ConfigChat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GroupId(id) => write!(f, "group={id}"),
            Self::UserId(id) => write!(f, "user={id}"),
        }
    }
}

static BACKEND: Lazy<OnceCell<lagrange::LograngeOnebot>> = Lazy::new(OnceCell::new);

pub struct Notifier {
    params: ConfigParams,
}

impl PlatformTrait for Notifier {
    fn metadata(&self) -> PlatformMetadata {
        PlatformMetadata { display_name: "QQ" }
    }
}

impl NotifierTrait for Notifier {
    fn notify<'a>(
        &'a self,
        notification: &'a Notification<'_>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(self.notify_impl(notification))
    }
}

impl Notifier {
    pub fn new(params: ConfigParams) -> Self {
        Self { params }
    }

    async fn notify_impl(&self, notification: &Notification<'_>) -> anyhow::Result<()> {
        info!("notifying to '{}'", self.params);

        match &notification.kind {
            NotificationKind::LiveOnline(live_status) => {
                self.notify_live(live_status, notification.source).await
            }
            NotificationKind::LiveTitle(live_status, _old_title) => {
                self.notify_live_title(live_status, notification.source)
                    .await
            }
            NotificationKind::Posts(posts) => self.notify_posts(posts, notification.source).await,
            NotificationKind::Log(message) => self.notify_log(message).await,
        }
    }

    async fn notify_live(
        &self,
        live_status: &LiveStatus,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        if !self.params.notifications.live_online {
            info!("live_online notification is disabled, skip notifying");
            return Ok(());
        }

        if live_status.online {
            let message = lagrange::Message::builder()
                .image(&live_status.cover_image_url)
                .text(format!(
                    "[{}] 🟢 {}\n{}",
                    source.platform.display_name, live_status.title, live_status.live_url
                ))
                .build();

            BACKEND
                .get()
                .unwrap()
                .send_message(&self.params.chat, message)
                .await?;
        }

        Ok(())
    }

    async fn notify_live_title(
        &self,
        live_status: &LiveStatus,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        if !self.params.notifications.live_title {
            info!("live_title notification is disabled, skip notifying");
            return Ok(());
        }

        let message = lagrange::Message::text(format!(
            "[{}] ✏️ {}",
            source.platform.display_name, live_status.title
        ));

        BACKEND
            .get()
            .unwrap()
            .send_message(&self.params.chat, message)
            .await?;

        Ok(())
    }

    async fn notify_posts(
        &self,
        posts: &PostsRef<'_>,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        if !self.params.notifications.post {
            info!("post notification is disabled, skip notifying");
            return Ok(());
        }

        let mut errors = vec![];

        for post in &posts.0 {
            if let Err(err) = self.notify_post(post, source).await {
                error!("failed to notify post to QQ: {err}");
                errors.push(err);
            }
        }

        errors
            .into_iter()
            .fold(Ok(()), |res, err| bail!("{res:?} {err}"))
    }

    async fn notify_post(&self, post: &Post, source: &StatusSource) -> anyhow::Result<()> {
        let mut content = String::new();

        write!(content, "[{}] ", source.platform.display_name)?;

        match &post.repost_from {
            Some(RepostFrom::Recursion(repost_from)) => {
                if !post.content.is_empty() {
                    writeln!(content, "💬 {}\n", post.content)?;
                }

                content.write_str("🔁 ")?;

                if let Some(user) = &repost_from.user {
                    write!(content, "{}: {}", user.nickname, repost_from.content)?;
                } else {
                    write!(content, "{}", repost_from.content)?;
                }
            }
            Some(RepostFrom::Legacy {
                is_repost,
                is_quote,
            }) => {
                if *is_repost {
                    content.write_str("🔁 ")?
                } else if *is_quote {
                    content.write_str("🔁💬 ")?
                }
                content.write_str(&post.content)?
            }
            None => content.write_str(&post.content)?,
        }
        content.write_str("\n")?;
        for url in post.urls.iter().into_iter() {
            write!(content, "\n{}: {}", url.display, url.url)?;
        }

        let images = post
            .attachments_recursive()
            .into_iter()
            .filter_map(|attachment| match attachment {
                PostAttachment::Image(image) => Some(image.media_url.as_str()),
                PostAttachment::Video(_) => None, // TODO: Handle videos
            });

        let message = lagrange::Message::builder()
            .images(images)
            .text(content)
            .build();

        BACKEND
            .get()
            .unwrap()
            .send_message(&self.params.chat, message)
            .await?;

        Ok(())
    }

    async fn notify_log(&self, message: &str) -> anyhow::Result<()> {
        if !self.params.notifications.log {
            info!("log notification is disabled, skip notifying");
            return Ok(());
        }

        let message = lagrange::Message::text(message);

        BACKEND
            .get()
            .unwrap()
            .send_message(&self.params.chat, message)
            .await?;

        Ok(())
    }
}
