pub mod lagrange;

use std::{
    borrow::Cow,
    collections::HashMap,
    fmt::{self, Write},
    future::Future,
    pin::Pin,
};

use anyhow::{anyhow, ensure};
use serde::Deserialize;
use spdlog::prelude::*;

use crate::{
    config::{self, Config},
    notify::NotifierTrait,
    platform::{PlatformMetadata, PlatformTrait},
    source::{
        LiveStatus, LiveStatusKind, Notification, NotificationKind, Post, PostAttachment, PostsRef,
        RepostFrom, StatusSource,
    },
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigGlobal {
    pub account: HashMap<String, ConfigAccount>,
}

impl config::Validator for ConfigGlobal {
    fn validate(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigAccount {
    pub lagrange: lagrange::ConfigLagrange,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigParams {
    #[serde(default)]
    pub notifications: config::Notifications,
    #[serde(flatten)]
    pub chat: ConfigChat,
    #[serde(default)]
    pub mention_all: bool,
    pub from: String,
}

impl config::Validator for ConfigParams {
    fn validate(&self) -> anyhow::Result<()> {
        let _account = config::Config::global()
            .platform()
            .qq
            .as_ref()
            .ok_or_else(|| anyhow!("QQ in global is missing"))?
            .account
            .get(&self.from)
            .ok_or_else(|| anyhow!("QQ account '{}' is not configured", self.from))?;
        ensure!(
            !self.mention_all || matches!(self.chat, ConfigChat::GroupId(_)),
            "mention_all can only be enabled for group chat"
        );
        Ok(())
    }
}

impl fmt::Display for ConfigParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "QQ:{},as={}", self.chat, self.from)?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigOverride {
    pub notifications: Option<config::NotificationsOverride>,
    #[serde(flatten)]
    pub chat: Option<ConfigChat>,
    pub mention_all: Option<bool>,
    pub from: Option<String>,
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
            mention_all: new.mention_all.unwrap_or(self.mention_all),
            from: new.from.unwrap_or(self.from),
        }
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

pub struct Notifier {
    params: config::Accessor<ConfigParams>,
    backend: lagrange::LagrangeOnebot<'static>,
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
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(self.notify_impl(notification))
    }
}

impl Notifier {
    pub fn new(params: config::Accessor<ConfigParams>) -> Self {
        let lagrange = lagrange::LagrangeOnebot::new(
            &Config::global()
                .platform()
                .qq
                .as_ref()
                .unwrap()
                .account
                .get(&params.from)
                .unwrap()
                .lagrange,
        );
        Self {
            params,
            backend: lagrange,
        }
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
            NotificationKind::Playback(_) => unimplemented!(),
            NotificationKind::Document(_) => unimplemented!(),
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

        if let LiveStatusKind::Online { start_time: _ } = live_status.kind {
            let message = lagrange::Message::builder()
                .image(&live_status.cover_image_url)
                .text(format!(
                    "[{}] 🟢 {}{}\n{}",
                    source.platform.display_name,
                    if self.params.notifications.author_name {
                        Cow::Owned(format!("[{}] ", live_status.streamer_name))
                    } else {
                        Cow::Borrowed("")
                    },
                    live_status.title,
                    live_status.live_url
                ))
                .mention_all_if(self.params.mention_all, true)
                .build();

            self.backend
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

        let message = lagrange::Message::builder()
            .text(format!(
                "[{}] ✏️ {}{}",
                source.platform.display_name,
                if self.params.notifications.author_name {
                    Cow::Owned(format!("[{}] ", live_status.streamer_name))
                } else {
                    Cow::Borrowed("")
                },
                live_status.title
            ))
            .mention_all_if(self.params.mention_all, true)
            .build();

        self.backend
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
                errors.push(err);
            }
        }
        ensure!(errors.is_empty(), "{errors:?}");
        Ok(())
    }

    async fn notify_post(&self, post: &Post, source: &StatusSource) -> anyhow::Result<()> {
        let mut content = String::new();

        write!(content, "[{}] ", source.platform.display_name)?;

        match &post.repost_from {
            Some(RepostFrom::Recursion(repost_from)) => {
                if !post.content.is_empty() {
                    write!(content, "💬 ")?;
                    if self.params.notifications.author_name {
                        write!(content, "{}: ", post.user.nickname)?;
                    }
                    writeln!(content, "{}\n", post.content.fallback())?;
                }

                content.write_str("🔁 ")?;
                write!(
                    content,
                    "{}: {}",
                    repost_from.user.nickname,
                    repost_from.content.fallback()
                )?;
            }
            None => {
                if self.params.notifications.author_name {
                    write!(content, "{}: ", post.user.nickname)?;
                }
                content.write_str(&post.content.fallback())?
            }
        }
        content.write_str("\n")?;
        for url in post
            .urls_recursive()
            .into_iter()
            .filter_map(|url| url.as_clickable())
        {
            write!(content, "\n{}: {}", url.display, url.url)?;
        }

        let images = post
            .attachments_recursive(true)
            .into_iter()
            .filter_map(|attachment| match attachment {
                PostAttachment::Image(image) => Some(image.media_url.as_str()),
                PostAttachment::Video(_) => None, // TODO: Handle videos
            });

        let message = lagrange::Message::builder()
            .images(images)
            .text(content)
            .mention_all_if(self.params.mention_all, true)
            .build();

        self.backend
            .send_message(&self.params.chat, message)
            .await?;

        Ok(())
    }

    async fn notify_log(&self, message: &str) -> anyhow::Result<()> {
        if !self.params.notifications.log {
            info!("log notification is disabled, skip notifying");
            return Ok(());
        }

        let message = lagrange::Message::builder()
            .text(message)
            .mention_all_if(self.params.mention_all, true)
            .build();

        self.backend
            .send_message(&self.params.chat, message)
            .await?;

        Ok(())
    }
}
