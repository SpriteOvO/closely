mod request;

use std::{
    borrow::Cow,
    collections::VecDeque,
    fmt,
    future::Future,
    pin::Pin,
    time::{Duration, SystemTime},
};

use anyhow::{anyhow, bail, ensure};
use humantime_serde::re::humantime;
use request::*;
use serde::Deserialize;
use spdlog::prelude::*;
use tokio::sync::Mutex;

use super::{ConfigChat, ConfigToken};
use crate::{
    config::{self, Accessor, AsSecretRef, Config, Overridable, Validator},
    helper,
    notify::NotifierTrait,
    platform::{PlatformMetadata, PlatformTrait},
    source::{
        DocumentRef, FileRef, LiveStatus, LiveStatusKind, Notification, NotificationKind,
        PlaybackFormat, PlaybackRef, Post, PostAttachment, PostUrl, PostsRef, RepostFrom,
        StatusSource,
    },
};

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

impl Validator for ConfigParams {
    fn validate(&self) -> anyhow::Result<()> {
        match &self.token {
            Some(token) => token.validate(),
            None => match Config::global()
                .platform()
                .telegram
                .as_ref()
                .and_then(|telegram| telegram.token.as_ref())
            {
                Some(token) => token.validate(),
                None => bail!("both token in global and notify are missing"),
            },
        }
    }
}

impl fmt::Display for ConfigParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "telegram:{}", self.chat)?;
        if let Some(thread_id) = self.thread_id {
            write!(f, ":({thread_id})")?;
        }
        Ok(())
    }
}

impl Overridable for ConfigParams {
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

pub struct Notifier {
    params: Accessor<ConfigParams>,
    current_live: Mutex<Option<CurrentLive>>,
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
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(self.notify_impl(notification))
    }
}

impl Notifier {
    pub fn new(params: Accessor<ConfigParams>) -> Self {
        Self {
            params,
            current_live: Mutex::new(None),
        }
    }

    fn token(&self) -> anyhow::Result<Cow<str>> {
        self.params
            .token
            .as_ref()
            .unwrap_or_else(|| {
                Config::global()
                    .platform()
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
            NotificationKind::Playback(playback) => {
                self.notify_playback(playback, notification.source).await
            }
            NotificationKind::Document(document) => {
                self.notify_document(document, notification.source).await
            }
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
            LiveStatusKind::Online { start_time } => {
                self.notify_live_online(live_status, source, start_time)
                    .await
            }
            LiveStatusKind::Offline | LiveStatusKind::Banned => {
                self.notify_live_offline(live_status, source).await
            }
        }
    }

    async fn notify_live_online(
        &self,
        live_status: &LiveStatus,
        source: &StatusSource,
        start_time: Option<SystemTime>,
    ) -> anyhow::Result<()> {
        let token = self.token()?;

        let title_history = VecDeque::from([live_status.title.clone()]);
        let start_time = start_time.unwrap_or_else(SystemTime::now);

        let text = make_live_text(
            self.params.notifications.author_name,
            &title_history,
            live_status,
            source,
            start_time,
        );
        let link_preview = LinkPreviewOwned::Above(live_status.cover_image_url.clone());
        let resp = Request::new(&token)
            .send_message(&self.params.chat, text)
            .thread_id_opt(self.params.thread_id)
            .link_preview(link_preview.as_ref())
            .send()
            .await
            .map_err(|err| anyhow!("failed to send request to Telegram: {err}"))?;
        ensure!(
            resp.ok,
            "response contains error, description '{}'",
            resp.description
                .unwrap_or_else(|| "*no description*".into())
        );

        *self.current_live.lock().await = Some(CurrentLive {
            start_time,
            // The doc guarantees `result` to be present if `ok` == `true`
            message_id: resp.result.unwrap().message_id,
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
        if let Some(current_live) = self.current_live.lock().await.take() {
            let token = self.token()?;

            let text = make_live_text(
                self.params.notifications.author_name,
                &current_live.title_history,
                live_status,
                source,
                current_live.start_time,
            );
            let resp = Request::new(&token)
                .edit_message_text(&self.params.chat, current_live.message_id, text)
                .link_preview(current_live.link_preview.as_ref())
                .send()
                .await
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
                "[{}] ✏️ {}{}",
                source.platform.display_name,
                if self.params.notifications.author_name {
                    Cow::Owned(format!("[{}] ", live_status.streamer_name))
                } else {
                    Cow::Borrowed("")
                },
                live_status.title
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
        if let Some(current_live) = self.current_live.lock().await.as_mut() {
            let token = self.token()?;

            current_live
                .title_history
                .push_front(live_status.title.clone());

            let text = make_live_text(
                self.params.notifications.author_name,
                &current_live.title_history,
                live_status,
                source,
                current_live.start_time,
            );
            let resp = Request::new(&token)
                .edit_message_text(&self.params.chat, current_live.message_id, text)
                .link_preview(current_live.link_preview.as_ref())
                .send()
                .await
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
                    text.push_plain("💬 ");
                    if self.params.notifications.author_name {
                        text.push_link(&post.user.nickname, &post.user.profile_url);
                        text.push_plain(": ");
                    }
                    text.push_content(&post.content);
                    text.push_plain("\n");
                }

                text.push_quote(|text| {
                    text.push_plain("🔁 ");

                    // In order for Telegram to display more relevant information about the
                    // post, we don't use `profile_url` here
                    //
                    // &repost_from.user.profile_url,
                    if let PostUrl::Clickable(url) = &repost_from.urls_recursive().major() {
                        text.push_link(&repost_from.user.nickname, &url.url);
                    } else {
                        text.push_plain(&repost_from.user.nickname);
                    }
                    text.push_plain(": ");
                    text.push_content(&repost_from.content);
                });
            }
            None => {
                if self.params.notifications.author_name {
                    text.push_link(&post.user.nickname, &post.user.profile_url);
                    text.push_plain(": ");
                }
                text.push_content(&post.content)
            }
        }

        const DISABLE_NOTIFICATION: bool = true; // TODO: Make it configurable

        let attachments = post.attachments_recursive(true);
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

    async fn notify_playback(
        &self,
        playback: &PlaybackRef<'_>,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        const WAIT_FOR: Duration = Duration::from_secs(60);

        for i in 0..3 {
            if let Err(err) = self.notify_playback_impl(playback, source, false).await {
                warn!(
                    "failed to notify playback '{playback}': {err}, wait for {} then retry",
                    humantime::format_duration(WAIT_FOR)
                );
                tokio::time::sleep(WAIT_FOR).await;
                warn!(
                    "notifying playback '{playback}' again, attempt {} of 3",
                    i + 1
                );
                continue;
            }
            return Ok(());
        }
        self.notify_playback_impl(playback, source, true)
            .await
            .inspect_err(|err| {
                error!("failed to notify playback '{playback}': {err}, this is the last attempt")
            })
    }

    // TODO: Parallel notify
    async fn notify_playback_impl(
        &self,
        playback: &PlaybackRef<'_>,
        source: &StatusSource,
        last_try: bool,
    ) -> anyhow::Result<()> {
        if !self.params.notifications.playback {
            info!("playback notification is disabled, skip notifying");
            return Ok(());
        }

        const FORMAT: PlaybackFormat = PlaybackFormat::Mp4;

        let playback = playback.get(FORMAT).await?;

        let token = self.token()?;

        // Send "uploading" message

        let resp = Request::new(&token)
            .send_message(
                &self.params.chat,
                make_file_text(
                    self.params.notifications.author_name,
                    FileUploadStage::PlaybackUploading,
                    &playback.file,
                    source,
                ),
            )
            .thread_id_opt(self.params.thread_id)
            .link_preview(LinkPreview::Disabled)
            .disable_notification() // TODO: Make it configurable
            .send()
            .await
            .map_err(|err| anyhow!("failed to send request to Telegram: {err}"))?;
        ensure!(
            resp.ok,
            "response contains error, description '{}'",
            resp.description
                .unwrap_or_else(|| "*no description*".into())
        );

        // Edit the media

        trace!("uploading playback to Telegram '{}'", playback.file);

        let edit_media = async || {
            let resp = Request::new(&token)
                .edit_message_media(
                    &self.params.chat,
                    resp.result.as_ref().unwrap().message_id,
                    Media::Video(MediaVideo {
                        input: MediaInput::Memory {
                            data: playback.file.data.clone(),
                            filename: Some(&playback.file.name),
                        },
                        resolution: Some(playback.resolution),
                        has_spoiler: false,
                    }),
                )
                .text(make_file_text(
                    self.params.notifications.author_name,
                    FileUploadStage::PlaybackFinished,
                    &playback.file,
                    source,
                ))
                .prefer_self_host()
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
        };

        let ret = edit_media().await;
        trace!(
            "finished uploading playback to Telegram '{}'",
            playback.file
        );

        if let Err(err) = ret {
            let message_id = resp.result.unwrap().message_id;
            if last_try {
                _ = Request::new(&token)
                    .edit_message_text(
                        &self.params.chat,
                        message_id,
                        make_file_text(
                            self.params.notifications.author_name,
                            FileUploadStage::PlaybackFailed,
                            &playback.file,
                            source,
                        ),
                    )
                    .send()
                    .await;
            } else {
                _ = Request::new(&token)
                    .delete_message(&self.params.chat, message_id)
                    .send()
                    .await;
            }
            Err(err)
        } else {
            Ok(())
        }
    }

    async fn notify_document(
        &self,
        document: &DocumentRef<'_>,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        if !self.params.notifications.document {
            info!("document notification is disabled, skip notifying");
            return Ok(());
        }

        let token = self.token()?;

        let resp = Request::new(&token)
            .send_document(
                &self.params.chat,
                MediaDocument {
                    input: MediaInput::Memory {
                        data: document.file.data.clone(),
                        filename: Some(&document.file.name),
                    },
                },
            )
            .text(make_file_text(
                self.params.notifications.author_name,
                FileUploadStage::MetadataFinished,
                &document.file,
                source,
            ))
            .thread_id_opt(self.params.thread_id)
            .disable_notification() // TODO: Make it configurable
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
    author_name: bool,
    title_history: impl IntoIterator<Item = &'a String>,
    live_status: &'a LiveStatus,
    source: &StatusSource,
    start_time: SystemTime,
) -> Text<'a> {
    let text = format!(
        "[{}] {} {}{}{}",
        source.platform.display_name,
        match live_status.kind {
            LiveStatusKind::Online { start_time: _ } => "🟢",
            LiveStatusKind::Offline => "🟠",
            LiveStatusKind::Banned => "🔴",
        },
        if author_name {
            Cow::Owned(format!("[{}] ", live_status.streamer_name))
        } else {
            Cow::Borrowed("")
        },
        itertools::join(title_history, " ⬅️ "),
        if live_status.kind == LiveStatusKind::Offline || live_status.kind == LiveStatusKind::Banned
        {
            if let Ok(dur) = start_time.elapsed() {
                Cow::Owned(format!(" ({})", helper::format_duration_in_min(dur)))
            } else {
                Cow::Borrowed("")
            }
        } else {
            Cow::Borrowed("")
        },
    );
    Text::link(text, &live_status.live_url)
}

enum FileUploadStage {
    PlaybackUploading,
    PlaybackFinished,
    PlaybackFailed,
    MetadataFinished,
}

fn make_file_text<'a>(
    _author_name: bool,
    stage: FileUploadStage,
    file: &FileRef<'a>,
    source: &'a StatusSource,
) -> Text<'a> {
    let emoji = match stage {
        FileUploadStage::PlaybackUploading => "⏳",
        FileUploadStage::PlaybackFinished => "🎥",
        FileUploadStage::PlaybackFailed => "❌",
        FileUploadStage::MetadataFinished => "📊",
    };
    // TODO: Append author_name
    let mut text = Text::plain(format!(
        "[{}] {emoji} {}",
        source.platform.display_name, file.name,
    ));
    match stage {
        FileUploadStage::PlaybackUploading | FileUploadStage::PlaybackFailed => {
            text.push_plain(format!(
                " ({})",
                humansize::format_size(file.size, humansize::BINARY)
            ));
        }
        _ => {}
    }
    text
}

struct CurrentLive {
    start_time: SystemTime,
    message_id: i64,
    link_preview: LinkPreviewOwned,
    // The first is the current title, the last is the oldest title
    title_history: VecDeque<String>,
}

struct NotifyPlaybackRetry {
    //
}
