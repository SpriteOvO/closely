mod request;

use std::{
    borrow::Cow,
    collections::{HashMap, VecDeque},
    fmt,
    future::Future,
    pin::Pin,
    sync::Arc,
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
    format_if,
    helper::{self, MaybeOwned},
    notify::{NotifierShared, NotifierTrait, SharedManager},
    platform::{PlatformMetadata, PlatformTraitStatic},
    source::{
        DocumentRef, Feed, FeedsRef, FileRef, LiveStatus, LiveStatusKind, Notification,
        NotificationKind, PlaybackFormat, PlaybackRef, Post, PostAttachment, PostPlatformUniqueId,
        PostUrl, PostsRef, RepostFrom, StatusSource,
    },
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct OptionExt {
    // For channels with comments enabled, messages sent with buttons will hide the comment
    // entrance, this option disables sending messages with buttons.
    #[serde(default = "helper::refl_bool::<false>")]
    pub no_button: bool,
}

impl Default for OptionExt {
    fn default() -> Self {
        helper::serde_default()
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct OptionExtOverride {
    pub no_button: Option<bool>,
}

impl Overridable for OptionExt {
    type Override = OptionExtOverride;

    fn override_into(self, new: Self::Override) -> Self {
        Self {
            no_button: new.no_button.unwrap_or(self.no_button),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigParams {
    #[serde(default, flatten)]
    pub base: config::NotifierBase<OptionExt>,
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
            base: match new.base {
                Some(base) => self.base.override_into(base),
                None => self.base,
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
    #[serde(flatten)]
    pub base: Option<config::NotifierBaseOverride<OptionExtOverride>>,
    #[serde(flatten)]
    pub chat: Option<ConfigChat>,
    pub thread_id: Option<i64>,
    #[serde(flatten)]
    token: Option<ConfigToken>,
}

static SHARED_MANAGER: SharedManager<SharedStates> = SharedManager::new();

#[derive(Default)]
pub struct SharedStates {
    sent_posts: HashMap<PostPlatformUniqueId, i64>,
}

impl NotifierShared for SharedStates {
    type Notifier = Notifier;
    type ConfigParams = ConfigParams;

    fn params_key(params: &Self::ConfigParams) -> String {
        format!("{}-{:?}", params.chat, params.thread_id)
    }
}

pub struct Notifier {
    params: Accessor<ConfigParams>,
    shared: Arc<Mutex<SharedStates>>,
    current_live: Mutex<Option<CurrentLive>>,
}

impl PlatformTraitStatic for Notifier {
    fn metadata() -> PlatformMetadata {
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
            shared: SHARED_MANAGER.obtain(&*params),
            params,
            current_live: Mutex::new(None),
        }
    }

    fn token(&self) -> anyhow::Result<Cow<'_, str>> {
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
        info!("notifying to", kv: { to: = self.params });

        match &notification.kind {
            NotificationKind::LiveOnline(live_status) => {
                self.notify_live(live_status, notification.source).await
            }
            NotificationKind::LiveTitle(live_status, _old_title) => {
                self.notify_live_title(live_status, notification.source)
                    .await
            }
            NotificationKind::Posts(posts) => self.notify_posts(posts, notification.source).await,
            NotificationKind::Feeds(feeds) => self.notify_feeds(feeds, notification.source).await,
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
        if !self.params.base.switch.live_online {
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
            self.params.base.option.platform_name,
            self.params.base.option.author_name,
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
                self.params.base.option.platform_name,
                self.params.base.option.author_name,
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
        if !self.params.base.switch.live_title {
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
                "{}✏️ {}{}",
                format_if!(
                    self.params.base.option.platform_name,
                    "[{}] ",
                    source.platform.display_name
                ),
                format_if!(
                    self.params.base.option.author_name,
                    "[{}] ",
                    live_status.streamer_name
                ),
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
                self.params.base.option.platform_name,
                self.params.base.option.author_name,
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
        if !self.params.base.switch.post {
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

    async fn truncate_conversation<'a>(
        &self,
        current_post: &'a Post,
        sent_posts: &mut HashMap<PostPlatformUniqueId, i64>,
    ) -> (MaybeOwned<'a, Post>, Option<i64>) {
        if !current_post
            .repost_chain()
            .any(|repost| sent_posts.contains_key(&repost.post.platform_unique_id()))
        {
            return (MaybeOwned::Borrowed(current_post), None);
        }

        let mut current_post = current_post.clone();

        let mut post = &mut current_post;
        while post.repost_from.is_some() {
            if let Some(msg_id) =
                sent_posts.get(&post.repost_from.as_ref().unwrap().post.platform_unique_id())
            {
                post.repost_from = None; // Cut the repost chain here
                return (MaybeOwned::Owned(current_post), Some(*msg_id));
            } else {
                post = &mut post.repost_from.as_mut().unwrap().post;
            }
        }

        (MaybeOwned::Owned(current_post), None)
    }

    async fn notify_post(
        &self,
        token: &str,
        post: &Post,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        let mut shared = self.shared.lock().await;
        let sent_posts = &mut shared.sent_posts;
        let (post, reply_to) = self.truncate_conversation(post, sent_posts).await;
        let post = post.as_ref();

        let mut text = Text::plain(format_if!(
            self.params.base.option.platform_name,
            "[{}] ",
            source.platform.display_name
        ));

        match &post.repost_from {
            Some(repost_from) => {
                if !post.content.is_empty() {
                    text.push_plain(if !post.prefer_treat_as_reply {
                        "💬 "
                    } else {
                        "🗣 "
                    });
                    if self.params.base.option.author_name {
                        text.push_link(&post.user.nickname, &post.user.profile_url);
                        text.push_plain(": ");
                    }
                    text.push_content(&post.content);
                    text.push_plain("\n");
                }

                fn push_reposts_rec<'a>(text: &mut Text<'a>, repost_from: &'a RepostFrom) {
                    text.push_quote(|text| {
                        text.push_plain("🔁 ");

                        // In order for Telegram to display more relevant information about the
                        // post, we don't use `profile_url` here
                        //
                        // &repost_from.post.user.profile_url,
                        if let PostUrl::Clickable(url) = &repost_from.post.urls_recursive().major()
                        {
                            text.push_link(&repost_from.post.user.nickname, &url.url);
                        } else {
                            text.push_plain(&repost_from.post.user.nickname);
                        }
                        text.push_plain(": ");
                        text.push_content(&repost_from.post.content);
                    });
                    if let Some(repost_from) = &repost_from.post.repost_from {
                        text.push_plain("\n");
                        push_reposts_rec(text, repost_from);
                    }
                }
                push_reposts_rec(&mut text, repost_from);
            }
            None => {
                if self.params.base.option.author_name {
                    text.push_link(&post.user.nickname, &post.user.profile_url);
                    text.push_plain(": ");
                }
                text.push_content(&post.content)
            }
        }

        const DISABLE_NOTIFICATION: bool = true; // TODO: Make it configurable

        let attachments = post.attachments_recursive(true);
        let num_attachments = attachments.len();

        // Jump buttons
        let buttons = if !self.params.base.option.ext.no_button
            && (num_attachments == 0 || num_attachments == 1)
        {
            Some(Markup::InlineKeyboard(vec![post
                .urls_recursive()
                .into_iter()
                .filter_map(|url| url.as_clickable())
                .map(|url| Button::new_url(&url.display, &url.url))
                .collect::<Vec<_>>()]))
        } else {
            text.push_plain("\n\n");
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
            None
        };

        let resp = match num_attachments {
            0 | 1 => {
                if num_attachments == 0 {
                    Request::new(token)
                        .send_message(&self.params.chat, text)
                        .reply_to_opt(reply_to)
                        .thread_id_opt(self.params.thread_id)
                        .disable_notification_bool(DISABLE_NOTIFICATION)
                        .markup_opt(buttons)
                        .link_preview(LinkPreview::Disabled)
                        .send()
                        .await
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
                    .reply_to_opt(reply_to)
                    .thread_id_opt(self.params.thread_id)
                    .disable_notification_bool(DISABLE_NOTIFICATION)
                    .markup_opt(buttons)
                    .send()
                    .await
                }
                .map(|resp| resp.map_result(|r| Some(r.message_id)))
            }
            _ => {
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
                    .map(|resp| resp.map_result(|r| r.first().map(|r| r.message_id)))
            }
        }
        .map_err(|err| anyhow!("failed to send request to Telegram: {err}"))?;

        ensure!(
            resp.ok,
            "response contains error, description '{}'",
            resp.description
                .unwrap_or_else(|| "*no description*".into())
        );

        if let Some(message_id) = resp.result.unwrap() {
            sent_posts.insert(post.platform_unique_id(), message_id);
        }
        Ok(())
    }

    async fn notify_feeds(
        &self,
        feeds: &FeedsRef<'_>,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        if !self.params.base.switch.feed {
            info!("feed notification is disabled, skip notifying");
            return Ok(());
        }

        let token = self.token()?;

        let mut errors = vec![];
        for feed in &feeds.items {
            if let Err(err) = self
                .notify_feed(token.as_ref(), feeds.title, feed, source)
                .await
            {
                errors.push(err);
            }
        }
        ensure!(errors.is_empty(), "{errors:?}");
        Ok(())
    }

    async fn notify_feed(
        &self,
        token: &str,
        title: Option<&str>,
        feed: &Feed,
        source: &StatusSource,
    ) -> anyhow::Result<()> {
        let mut text = Text::plain(format_if!(
            self.params.base.option.platform_name,
            "[{}] ",
            source.platform.display_name
        ));

        if let Some(title) = title {
            text.push_plain(format!("📢 {title}\n\n"));
        }

        if let Some(title) = &feed.title {
            if let Some(link) = feed.link.as_deref() {
                text.push_link(title, link);
            } else {
                text.push_plain(title);
            }
            text.push_plain("\n\n");
        }

        // TODO: Description

        const DISABLE_NOTIFICATION: bool = true; // TODO: Make it configurable

        let resp = Request::new(token)
            .send_message(&self.params.chat, text)
            .thread_id_opt(self.params.thread_id)
            .disable_notification_bool(DISABLE_NOTIFICATION)
            // .link_preview(LinkPreview::Disabled)
            .send()
            .await
            .map(|resp| resp.map_result(|r| Some(r.message_id)))
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
        if !self.params.base.switch.log {
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
                let wait_for_fmt = humantime::format_duration(WAIT_FOR);
                warn!("failed to notify playback, wait for {wait_for_fmt} then retry", kv: { err:, playback:, duration: = wait_for_fmt});
                tokio::time::sleep(WAIT_FOR).await;
                warn!("notifying playback '{playback}' again, attempt {} of 3", i + 1, kv: { attempt = i + 1 });
                continue;
            }
            return Ok(());
        }
        self.notify_playback_impl(playback, source, true)
            .await
            .inspect_err(|err| {
                error!("failed to notify playback, this is the last attempt", kv: { err:, playback: })
            })
    }

    // TODO: Parallel notify
    async fn notify_playback_impl(
        &self,
        playback: &PlaybackRef<'_>,
        source: &StatusSource,
        last_try: bool,
    ) -> anyhow::Result<()> {
        if !self.params.base.switch.playback {
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
                    self.params.base.option.platform_name,
                    self.params.base.option.author_name,
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

        trace!("uploading playback to Telegram", kv: { file: = playback.file });

        let edit_media = async || {
            let resp = Request::new(&token)
                .edit_message_media(
                    &self.params.chat,
                    resp.result.as_ref().unwrap().message_id,
                    Media::Video(MediaVideo {
                        input: MediaInput::Memory {
                            data: playback.file.data.clone(),
                            filename: Some(Cow::Borrowed(&playback.file.name)),
                        },
                        resolution: Some(playback.resolution),
                        has_spoiler: false,
                    }),
                )
                .text(make_file_text(
                    self.params.base.option.platform_name,
                    self.params.base.option.author_name,
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
        trace!("finished uploading playback to Telegram", kv: { file: = playback.file });

        if let Err(err) = ret {
            let message_id = resp.result.unwrap().message_id;
            if last_try {
                _ = Request::new(&token)
                    .edit_message_text(
                        &self.params.chat,
                        message_id,
                        make_file_text(
                            self.params.base.option.platform_name,
                            self.params.base.option.author_name,
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
        if !self.params.base.switch.document {
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
                        filename: Some(Cow::Borrowed(&document.file.name)),
                    },
                },
            )
            .text(make_file_text(
                self.params.base.option.platform_name,
                self.params.base.option.author_name,
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
    platform_name: bool,
    author_name: bool,
    title_history: impl IntoIterator<Item = &'a String>,
    live_status: &'a LiveStatus,
    source: &StatusSource,
    start_time: SystemTime,
) -> Text<'a> {
    let text = format!(
        "{}{} {}{}{}",
        format_if!(platform_name, "[{}] ", source.platform.display_name),
        match live_status.kind {
            LiveStatusKind::Online { start_time: _ } => "🟢",
            LiveStatusKind::Offline => "🟠",
            LiveStatusKind::Banned => "🔴",
        },
        format_if!(author_name, "[{}] ", live_status.streamer_name),
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
    platform_name: bool,
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
        "{}{emoji} {}",
        format_if!(platform_name, "[{}] ", source.platform.display_name),
        file.name,
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
