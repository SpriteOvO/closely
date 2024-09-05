mod request;

use std::{borrow::Cow, collections::VecDeque, fmt, future::Future, pin::Pin};

use anyhow::{anyhow, bail, ensure};
use request::*;
use serde::Deserialize;
use serde_json as json;
use spdlog::prelude::*;
use tokio::sync::Mutex;

use super::NotifierTrait;
use crate::{
    config::{self, AsSecretRef, Config},
    helper,
    platform::{PlatformMetadata, PlatformTrait},
    secret_enum, serde_impl_default_for,
    source::{
        LiveStatus, LiveStatusKind, Notification, NotificationKind, Post, PostAttachment, PostUrl,
        PostsRef, RepostFrom, StatusSource,
    },
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigGlobal {
    #[serde(flatten)]
    pub token: Option<ConfigToken>,
    #[serde(default)]
    pub experimental: ConfigExperimental,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigExperimental {
    #[serde(default = "helper::refl_bool::<false>")]
    pub send_live_image_as_preview: bool,
}

serde_impl_default_for!(ConfigExperimental);

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigParams {
    #[serde(default)]
    pub notifications: config::Notifications,
    #[serde(flatten)]
    pub chat: ConfigChat,
    pub thread_id: Option<i64>,
    #[serde(flatten)]
    pub token: Option<ConfigToken>,
}

impl ConfigParams {
    pub fn validate(&self, global: &config::PlatformGlobal) -> anyhow::Result<()> {
        match &self.token {
            Some(token) => token.as_secret_ref().validate(),
            None => match global
                .telegram
                .as_ref()
                .and_then(|telegram| telegram.token.as_ref())
            {
                Some(token) => token.as_secret_ref().validate(),
                None => bail!("both token in global and notify are missing"),
            },
        }
    }
}

impl fmt::Display for ConfigParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "telegram:{}", self.chat)?;
        if let Some(thread_id) = self.thread_id {
            write!(f, ":({})", thread_id)?;
        }
        Ok(())
    }
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
            thread_id: new.thread_id.or(self.thread_id),
            token: new.token.or(self.token),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigOverride {
    pub notifications: Option<config::NotificationsOverride>,
    #[serde(flatten)]
    pub chat: Option<ConfigChat>,
    pub thread_id: Option<i64>,
    #[serde(flatten)]
    token: Option<ConfigToken>,
}

secret_enum! {
    #[derive(Clone, Debug, PartialEq, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ConfigToken {
        Token(String),
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigChat {
    Id(i64),
    Username(String),
}

impl ConfigChat {
    fn to_json(&self) -> json::Value {
        match self {
            ConfigChat::Id(id) => json::Value::Number((*id).into()),
            ConfigChat::Username(username) => json::Value::String(format!("@{username}")),
        }
    }
}

impl fmt::Display for ConfigChat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigChat::Id(id) => write!(f, "{}", id),
            ConfigChat::Username(username) => write!(f, "@{}", username),
        }
    }
}

pub struct Notifier {
    params: ConfigParams,
    last_live_message: Mutex<Option<SentMessage>>,
}

impl PlatformTrait for Notifier {
    fn metadata(&self) -> PlatformMetadata {
        PlatformMetadata {
            display_name: "Telegram",
        }
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
        Self {
            params,
            last_live_message: Mutex::new(None),
        }
    }

    fn exp_send_live_image_as_preview(&self) -> bool {
        Config::platform_global()
            .telegram
            .as_ref()
            .map(|telegram| telegram.experimental.send_live_image_as_preview)
            .unwrap_or_default()
    }

    fn token(&self) -> anyhow::Result<Cow<str>> {
        self.params
            .token
            .as_ref()
            .unwrap_or_else(|| {
                Config::platform_global()
                    .telegram
                    .as_ref()
                    .unwrap()
                    .token
                    .as_ref()
                    .unwrap()
            })
            .as_secret_ref()
            .get_str()
            .map_err(|err| anyhow!("failed to read token for telegram: {err}"))
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

        match live_status.kind {
            LiveStatusKind::Online => self.notify_live_online(live_status, source).await,
            LiveStatusKind::Offline | LiveStatusKind::Banned => {
                self.notify_live_offline(live_status, source).await
            }
        }
    }

    async fn notify_live_online(
        &self,
        live_status: &LiveStatus,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        let token = self.token()?;

        let title_history = VecDeque::from([live_status.title.clone()]);

        let text = make_live_text(&title_history, live_status, source);
        let (resp, link_preview) = if !self.exp_send_live_image_as_preview() {
            let link_preview = LinkPreviewOwned::Disabled;
            (
                Request::new(&token)
                    .send_photo(
                        &self.params.chat,
                        MediaPhoto {
                            url: &live_status.cover_image_url,
                            has_spoiler: false,
                        },
                    )
                    .thread_id_opt(self.params.thread_id)
                    .text(text)
                    .send()
                    .await,
                link_preview,
            )
        } else {
            let link_preview = LinkPreviewOwned::Above(live_status.cover_image_url.clone());
            (
                Request::new(&token)
                    .send_message(&self.params.chat, text)
                    .thread_id_opt(self.params.thread_id)
                    .link_preview(link_preview.as_ref())
                    .send()
                    .await,
                link_preview,
            )
        };
        let resp = resp.map_err(|err| anyhow!("failed to send request to Telegram: {err}"))?;
        ensure!(
            resp.ok,
            "response contains error, description '{}'",
            resp.description
                .unwrap_or_else(|| "*no description*".into())
        );

        *self.last_live_message.lock().await = Some(SentMessage {
            // The doc guarantees `result` to be present if `ok` == `true`
            id: resp.result.unwrap().message_id,
            link_preview,
            title_history,
        });

        Ok(())
    }

    async fn notify_live_offline(
        &self,
        live_status: &LiveStatus,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        if let Some(last_live_message) = self.last_live_message.lock().await.take() {
            let token = self.token()?;

            let text = make_live_text(&last_live_message.title_history, live_status, source);
            let resp = if !self.exp_send_live_image_as_preview() {
                Request::new(&token)
                    .edit_message_caption(&self.params.chat, last_live_message.id)
                    .text(text)
                    .send()
                    .await
            } else {
                Request::new(&token)
                    .edit_message_text(&self.params.chat, last_live_message.id, text)
                    .link_preview(last_live_message.link_preview.as_ref())
                    .send()
                    .await
            }
            .map_err(|err| anyhow!("failed to send request to Telegram: {err}"))?;
            ensure!(
                resp.ok,
                "response contains error, description '{}'",
                resp.description
                    .unwrap_or_else(|| "*no description*".into())
            );
        }
        Ok(())
    }

    async fn notify_live_title(
        &self,
        live_status: &LiveStatus,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        // Update the last message
        self.notify_live_title_update(live_status, source).await?;

        // Send a new message
        if !self.params.notifications.live_title {
            info!("live_title notification is disabled, skip notifying");
            return Ok(());
        }
        self.notify_live_title_send(live_status, source).await
    }

    async fn notify_live_title_send(
        &self,
        live_status: &LiveStatus,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        let token = self.token()?;

        let text = Text::link(
            format!(
                "[{}] ✏️ {}",
                source.platform.display_name, live_status.title
            ),
            &live_status.live_url,
        );

        let resp = Request::new(&token)
            .send_message(&self.params.chat, text)
            .thread_id_opt(self.params.thread_id)
            // .disable_notification() // TODO: Make it configurable
            .link_preview(LinkPreview::Disabled)
            .send()
            .await
            .map_err(|err| anyhow!("failed to send request to Telegram: {err}"))?;
        ensure!(
            resp.ok,
            "response contains error, description '{}'",
            resp.description
                .unwrap_or_else(|| "*no description*".into())
        );

        Ok(())
    }

    async fn notify_live_title_update(
        &self,
        live_status: &LiveStatus,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        if let Some(last_live_message) = self.last_live_message.lock().await.as_mut() {
            let token = self.token()?;

            last_live_message
                .title_history
                .push_front(live_status.title.clone());

            let text = make_live_text(&last_live_message.title_history, live_status, source);
            let resp = if !self.exp_send_live_image_as_preview() {
                Request::new(&token)
                    .edit_message_caption(&self.params.chat, last_live_message.id)
                    .text(text)
                    .send()
                    .await
            } else {
                Request::new(&token)
                    .edit_message_text(&self.params.chat, last_live_message.id, text)
                    .link_preview(last_live_message.link_preview.as_ref())
                    .send()
                    .await
            }
            .map_err(|err| anyhow!("failed to send request to Telegram: {err}"))?;
            ensure!(
                resp.ok,
                "response contains error, description '{}'",
                resp.description
                    .unwrap_or_else(|| "*no description*".into())
            );
        }
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

        let token = self.token()?;

        let mut errors = vec![];
        for post in &posts.0 {
            if let Err(err) = self.notify_post(token.as_ref(), post, source).await {
                errors.push(err);
            }
        }
        ensure!(errors.is_empty(), "{errors:?}");
        Ok(())
    }

    async fn notify_post(
        &self,
        token: &str,
        post: &Post,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        let mut text = Text::plain(format!("[{}] ", source.platform.display_name));

        match &post.repost_from {
            Some(RepostFrom::Recursion(repost_from)) => {
                if !post.content.is_empty() {
                    text.push_plain(format!("💬 {}\n", post.content));
                }

                text.push_quote(|text| {
                    text.push_plain("🔁 ");

                    if let Some(user) = &repost_from.user {
                        // In order for Telegram to display more relevant information about the
                        // post, we don't use `profile_url` here
                        //
                        // &repost_from.user.profile_url,
                        if let PostUrl::Clickable(url) = &repost_from.urls_recursive().major() {
                            text.push_link(&user.nickname, &url.url);
                        } else {
                            text.push_plain(&user.nickname);
                        }
                        text.push_plain(": ");
                    }
                    text.push_plain(&repost_from.content);
                });
            }
            Some(RepostFrom::Legacy {
                is_repost,
                is_quote,
            }) => {
                if *is_repost {
                    text.push_plain("🔁 ")
                } else if *is_quote {
                    text.push_plain("🔁💬 ")
                }
                text.push_plain(&post.content)
            }
            None => text.push_plain(&post.content),
        }

        const DISABLE_NOTIFICATION: bool = true; // TODO: Make it configurable

        let attachments = post.attachments_recursive();
        let num_attachments = attachments.len();

        let resp = match num_attachments {
            0 | 1 => {
                // Jump buttons
                let buttons = vec![post
                    .urls_recursive()
                    .into_iter()
                    .filter_map(|url| url.as_clickable())
                    .map(|url| Button::new_url(&url.display, &url.url))
                    .collect::<Vec<_>>()];

                if num_attachments == 0 {
                    Request::new(token)
                        .send_message(&self.params.chat, text)
                        .thread_id_opt(self.params.thread_id)
                        .disable_notification_bool(DISABLE_NOTIFICATION)
                        .markup(Markup::InlineKeyboard(buttons))
                        .send()
                        .await
                        .map(|resp| resp.discard_result())
                } else {
                    let attachment = attachments.first().unwrap();

                    match attachment {
                        PostAttachment::Image(image) => {
                            // TODO: `sendAnimation` for single GIF?
                            Request::new(token).send_photo(&self.params.chat, image.into())
                        }
                        PostAttachment::Video(video) => {
                            Request::new(token).send_video(&self.params.chat, video.into())
                        }
                    }
                    .text(text)
                    .thread_id_opt(self.params.thread_id)
                    .disable_notification_bool(DISABLE_NOTIFICATION)
                    .markup(Markup::InlineKeyboard(buttons))
                    .send()
                    .await
                    .map(|resp| resp.discard_result())
                }
            }
            _ => {
                text.push_plain("\n\n");

                // Jump buttons
                {
                    let mut iter = post
                        .urls_recursive()
                        .into_iter()
                        .filter_map(|url| url.as_clickable())
                        .peekable();
                    while let Some(url) = iter.next() {
                        text.push_link(format!(">> {} <<", url.display), &url.url);
                        if iter.peek().is_some() {
                            text.push_plain(" | ");
                        }
                    }
                }

                let medias = attachments.iter().map(|attachment| match attachment {
                    // TODO: Mixing GIF in media group to send is not yet supported in Telegram, add
                    // an overlay like video? (see comment in twitter.com implementation)
                    PostAttachment::Image(image) => Media::Photo(image.into()),
                    PostAttachment::Video(video) => Media::Video(video.into()),
                });

                Request::new(token)
                    .send_media_group(&self.params.chat)
                    .medias(medias)
                    .text(text)
                    .thread_id_opt(self.params.thread_id)
                    .disable_notification_bool(DISABLE_NOTIFICATION)
                    .send()
                    .await
                    .map(|resp| resp.discard_result())
            }
        }
        .map_err(|err| anyhow!("failed to send request to Telegram: {err}"))?;

        ensure!(
            resp.ok,
            "response contains error, description '{}'",
            resp.description
                .unwrap_or_else(|| "*no description*".into())
        );

        Ok(())
    }

    async fn notify_log(&self, message: &str) -> anyhow::Result<()> {
        if !self.params.notifications.log {
            info!("log notification is disabled, skip notifying");
            return Ok(());
        }

        let token = self.token()?;

        let resp = Request::new(&token)
            .send_message(&self.params.chat, Text::plain(message))
            .thread_id_opt(self.params.thread_id)
            .link_preview(LinkPreview::Disabled)
            // .disable_notification() // TODO: Make it configurable
            .send()
            .await
            .map_err(|err| anyhow!("failed to send request to Telegram: {err}"))?;

        ensure!(
            resp.ok,
            "response contains error, description '{}'",
            resp.description
                .unwrap_or_else(|| "*no description*".into())
        );

        Ok(())
    }
}

fn make_live_text<'a>(
    title_history: impl IntoIterator<Item = &'a String>,
    live_status: &'a LiveStatus,
    source: &StatusSource,
) -> Text<'a> {
    let text = format!(
        "[{}] {} {}",
        source.platform.display_name,
        match live_status.kind {
            LiveStatusKind::Online => "🟢",
            LiveStatusKind::Offline => "🟠",
            LiveStatusKind::Banned => "🔴",
        },
        itertools::join(title_history, " ⬅️ "),
    );
    Text::link(text, &live_status.live_url)
}

struct SentMessage {
    id: i64,
    link_preview: LinkPreviewOwned,
    // The first is the current title, the last is the oldest title
    title_history: VecDeque<String>,
}
