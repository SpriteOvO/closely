use std::{borrow::Cow, io::Cursor, mem, ops::Range, time::Duration};

use anyhow::{anyhow, ensure};
use bytes::Bytes;
use http::Uri;
use image::{imageops::FilterType as ImageFilterType, GenericImageView};
use itertools::Itertools;
use once_cell::sync::Lazy;
use reqwest::multipart::{Form, Part};
use serde::{
    de::{DeserializeOwned, IgnoredAny},
    Deserialize,
};
use serde_json::{self as json, json};
use spdlog::prelude::*;

use super::{ConfigApiServer, ConfigChat};
use crate::{
    config::Config,
    helper::{self, VideoResolution},
    source::{PostAttachmentImage, PostAttachmentVideo, PostContent, PostContentPart},
};

pub struct Request<'a> {
    token: &'a str,
}

impl<'a> Request<'a> {
    pub fn new(token: &'a str) -> Self {
        Self { token }
    }

    async fn send_request<T: DeserializeOwned>(
        &self,
        method: &str,
        body: &json::Value,
        medias: impl IntoIterator<Item = Media<'_>>,
        prefer_self_host: bool,
    ) -> anyhow::Result<Response<T>> {
        self.send_request_inner(
            method,
            body,
            medias.into_iter().collect(),
            true,
            prefer_self_host,
        )
        .await
    }

    async fn send_request_force_download<T: DeserializeOwned>(
        &self,
        method: &str,
        body: &json::Value,
        medias: impl IntoIterator<Item = Media<'_>>,
        prefer_self_host: bool,
    ) -> anyhow::Result<Response<T>> {
        let downloaded = download_files(medias).await?;
        self.send_request_inner(method, body, downloaded, true, prefer_self_host)
            .await
    }

    async fn send_request_inner<T: DeserializeOwned>(
        &self,
        method: &str,
        body: &json::Value,
        // If `files` specified, the media fields in body should be replaced with
        // "attach://{index}"
        files: Vec<Media<'_>>,
        retry: bool,
        prefer_self_host: bool,
    ) -> anyhow::Result<Response<T>> {
        let mut client =
            helper::reqwest_client()?.post(make_api_url(self.token, method, prefer_self_host));

        let mem_files = files
            .clone()
            .into_iter()
            .enumerate()
            .filter(|(_, file)| matches!(file.input(), MediaInput::Memory { .. }))
            .collect::<Vec<_>>();
        if !mem_files.is_empty() {
            let form = form_append_json(Form::new(), body.as_object().unwrap());
            let form = mem_files.into_iter().try_fold(
                form,
                |mut form, (i, file)| -> anyhow::Result<Form> {
                    let is_photo = matches!(file, Media::Photo(_));
                    if let MediaInput::Memory { data, filename } = file.into_input() {
                        let mut part = media_into_part(i, data, is_photo)?;
                        if let Some(filename) = filename {
                            part = part.file_name(filename.to_string());
                        }
                        form = form.part(i.to_string(), part);
                    }
                    Ok(form)
                },
            )?;
            client = client
                .multipart(form)
                .timeout(Duration::from_secs(60 * 60 /* 1 hour */));
        } else {
            client = client.json(&body);
        }

        let resp = client
            .send()
            .await
            .map_err(|err| anyhow!("failed to send request: {err}"))?;

        let status = resp.status();
        let text = resp.text().await.map_err(|err| {
            anyhow!("failed to obtain text from response: {err}, status: {status}")
        })?;

        let resp: Response<T> = json::from_str(&text).map_err(|err| {
            anyhow!("failed to deserialize response: {err}, status: {status}, text: '{text}', request '{body}'")
        })?;

        if retry && !resp.ok && resp.description.is_some() {
            if let Some(after) = resp
                .description
                .as_deref()
                .unwrap()
                .strip_prefix("Too Many Requests: retry after ")
            {
                warn!("Telegram rate limited, retry after '{}' + 1 seconds", after);

                let after = after
                    .parse::<u64>()
                    .map_err(|err| anyhow!("failed to parse rate limit duration: {err}"))?;
                tokio::time::sleep(tokio::time::Duration::from_secs(after + 1)).await;

                return Box::pin(self.send_request_inner(
                    method,
                    body,
                    files,
                    false,
                    prefer_self_host,
                ))
                .await;
            }
        }

        Ok(resp)
    }

    pub fn send_message(self, chat: &'a ConfigChat, text: Text<'a>) -> SendMessage<'a> {
        SendMessage {
            base: self,
            chat,
            text,
            thread_id: None,
            disable_notification: false,
            link_preview: None,
            markup: None,
        }
    }

    pub fn send_photo(self, chat: &'a ConfigChat, photo: MediaPhoto<'a>) -> SendMedia<'a> {
        SendMedia {
            base: self,
            chat,
            media: Media::Photo(photo),
            thread_id: None,
            text: None,
            disable_notification: false,
            markup: None,
            prefer_self_host: false,
        }
    }

    pub fn send_video(self, chat: &'a ConfigChat, video: MediaVideo<'a>) -> SendMedia<'a> {
        SendMedia {
            base: self,
            chat,
            media: Media::Video(video),
            thread_id: None,
            text: None,
            disable_notification: false,
            markup: None,
            prefer_self_host: false,
        }
    }

    pub fn send_document(self, chat: &'a ConfigChat, document: MediaDocument<'a>) -> SendMedia<'a> {
        SendMedia {
            base: self,
            chat,
            media: Media::Document(document),
            thread_id: None,
            text: None,
            disable_notification: false,
            markup: None,
            prefer_self_host: false,
        }
    }

    pub fn send_media_group(self, chat: &'a ConfigChat) -> SendMediaGroup<'a> {
        SendMediaGroup {
            base: self,
            chat,
            medias: vec![],
            thread_id: None,
            text: None,
            disable_notification: false,
            prefer_self_host: false,
        }
    }

    pub fn edit_message_text(
        self,
        chat: &'a ConfigChat,
        message_id: i64,
        text: Text<'a>,
    ) -> EditMessageText<'a> {
        EditMessageText {
            base: self,
            chat,
            message_id,
            text,
            link_preview: None,
        }
    }

    pub fn edit_message_caption(
        self,
        chat: &'a ConfigChat,
        message_id: i64,
    ) -> EditMessageCaption<'a> {
        EditMessageCaption {
            base: self,
            chat,
            message_id,
            text: None,
        }
    }

    pub fn edit_message_media(
        self,
        chat: &'a ConfigChat,
        message_id: i64,
        media: Media<'a>,
    ) -> EditMessageMedia<'a> {
        EditMessageMedia {
            base: self,
            chat,
            message_id,
            text: None,
            media,
            prefer_self_host: false,
        }
    }
}

fn make_api_url(token: &str, method: &str, prefer_self_host: bool) -> String {
    static OFFICIAL: Lazy<Uri> = Lazy::new(|| "https://api.telegram.org".parse().unwrap());

    let url_opts = Config::global()
        .platform()
        .telegram
        .as_ref()
        .and_then(|t| t.api_server.as_ref())
        .map(|opts| match opts {
            ConfigApiServer::Url(url) => (url, false),
            ConfigApiServer::UrlOpts { url, as_necessary } => (url, *as_necessary),
        });
    let url = match url_opts {
        Some((url, as_necessary)) => {
            if !as_necessary || prefer_self_host {
                url
            } else {
                &*OFFICIAL
            }
        }
        None => &*OFFICIAL,
    };
    make_api_url_impl(url, token, method)
}

fn make_api_url_impl(url: &Uri, token: &str, method: &str) -> String {
    format!("{url}bot{token}/{method}")
}

pub enum ButtonKind<'a> {
    Url(&'a str),
}

pub struct Button<'a> {
    text: &'a str,
    kind: ButtonKind<'a>,
}

impl<'a> Button<'a> {
    pub fn new_url(text: &'a str, url: &'a str) -> Self {
        Self {
            text,
            kind: ButtonKind::Url(url),
        }
    }
}

pub enum Markup<'a> {
    InlineKeyboard(Vec<Vec<Button<'a>>>),
}

impl Markup<'_> {
    fn into_json(self) -> json::Value {
        match self {
            Markup::InlineKeyboard(buttons) => {
                let buttons = buttons
                    .into_iter()
                    .map(|row| {
                        row.into_iter()
                            .map(|button| match button.kind {
                                ButtonKind::Url(url) => {
                                    json!({
                                        "text": button.text,
                                        "url": url,
                                    })
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>();
                json!({"inline_keyboard": buttons})
            }
        }
    }
}

pub enum LinkPreviewOwned {
    Disabled,
    Below(String),
    Above(String),
}

impl LinkPreviewOwned {
    pub fn as_ref(&self) -> LinkPreview {
        match self {
            Self::Disabled => LinkPreview::Disabled,
            Self::Below(url) => LinkPreview::Below(url),
            Self::Above(url) => LinkPreview::Above(url),
        }
    }
}

pub enum LinkPreview<'a> {
    Disabled,
    Below(&'a str),
    Above(&'a str),
}

impl LinkPreview<'_> {
    fn into_json(self) -> json::Value {
        match self {
            Self::Disabled => {
                json!({
                    "is_disabled": true
                })
            }
            Self::Below(url) => {
                json!({
                    "is_disabled": false,
                    "url": url,
                    "prefer_large_media": true,
                    "show_above_text": false
                })
            }
            Self::Above(url) => {
                json!({
                    "is_disabled": false,
                    "url": url,
                    "prefer_large_media": true,
                    "show_above_text": true
                })
            }
        }
    }
}

pub struct SendMessage<'a> {
    base: Request<'a>,
    chat: &'a ConfigChat,
    text: Text<'a>,
    thread_id: Option<i64>,
    disable_notification: bool,
    link_preview: Option<LinkPreview<'a>>,
    markup: Option<Markup<'a>>,
}

impl<'a> SendMessage<'a> {
    pub fn disable_notification(self) -> Self {
        Self {
            disable_notification: true,
            ..self
        }
    }

    pub fn disable_notification_bool(self, value: bool) -> Self {
        Self {
            disable_notification: value,
            ..self
        }
    }

    pub fn link_preview(self, options: LinkPreview<'a>) -> Self {
        Self {
            link_preview: Some(options),
            ..self
        }
    }

    pub fn thread_id(self, thread_id: i64) -> Self {
        Self {
            thread_id: Some(thread_id),
            ..self
        }
    }

    pub fn thread_id_opt(self, thread_id: Option<i64>) -> Self {
        Self { thread_id, ..self }
    }

    pub fn markup(self, markup: Markup<'a>) -> Self {
        Self {
            markup: Some(markup),
            ..self
        }
    }

    pub async fn send(self) -> anyhow::Result<Response<ResultMessage>> {
        let mut body = json!(
            {
                "chat_id": self.chat.to_json(),
                "message_thread_id": self.thread_id,
                "disable_notification": self.disable_notification
            }
        );
        {
            let (text, entities) = self.text.into_json();
            let body = body.as_object_mut().unwrap();
            body.insert("text".into(), text);
            body.insert("entities".into(), entities);
        }
        if let Some(link_preview) = self.link_preview {
            body["link_preview_options"] = link_preview.into_json();
        }
        if let Some(markup) = self.markup {
            body["reply_markup"] = markup.into_json();
        }
        self.base
            .send_request("sendMessage", &body, [], false)
            .await
    }
}

#[derive(Clone, Debug)]
pub enum Media<'a> {
    Photo(MediaPhoto<'a>),
    Video(MediaVideo<'a>),
    Document(MediaDocument<'a>),
}

impl<'a> Media<'a> {
    fn input(&self) -> &MediaInput<'a> {
        match self {
            Self::Photo(photo) => &photo.input,
            Self::Video(video) => &video.input,
            Self::Document(document) => &document.input,
        }
    }

    fn input_mut(&mut self) -> &mut MediaInput<'a> {
        match self {
            Self::Photo(photo) => &mut photo.input,
            Self::Video(video) => &mut video.input,
            Self::Document(document) => &mut document.input,
        }
    }

    fn into_input(self) -> MediaInput<'a> {
        match self {
            Self::Photo(photo) => photo.input,
            Self::Video(video) => video.input,
            Self::Document(document) => document.input,
        }
    }
}

#[derive(Clone, Debug)]
pub enum MediaInput<'a> {
    Url(&'a str),
    Memory {
        data: Bytes,
        filename: Option<&'a str>,
    },
}

impl MediaInput<'_> {
    fn to_url(&self, index: usize) -> Cow<str> {
        match self {
            Self::Url(url) => Cow::Borrowed(url),
            Self::Memory { .. } => Cow::Owned(format!("attach://{index}")),
        }
    }
}

#[derive(Clone, Debug)]
pub struct MediaPhoto<'a> {
    pub input: MediaInput<'a>,
    pub has_spoiler: bool,
}

impl<'a> From<&'a PostAttachmentImage> for MediaPhoto<'a> {
    fn from(value: &'a PostAttachmentImage) -> Self {
        Self {
            input: MediaInput::Url(&value.media_url),
            has_spoiler: value.has_spoiler,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MediaVideo<'a> {
    pub input: MediaInput<'a>,
    pub resolution: Option<VideoResolution>,
    pub has_spoiler: bool,
}

impl<'a> From<&'a PostAttachmentVideo> for MediaVideo<'a> {
    fn from(value: &'a PostAttachmentVideo) -> Self {
        Self {
            input: MediaInput::Url(&value.media_url),
            resolution: None,
            has_spoiler: value.has_spoiler,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MediaDocument<'a> {
    pub input: MediaInput<'a>,
}

pub struct SendMedia<'a> {
    base: Request<'a>,
    chat: &'a ConfigChat,
    media: Media<'a>,
    thread_id: Option<i64>,
    text: Option<Text<'a>>,
    disable_notification: bool,
    markup: Option<Markup<'a>>,
    prefer_self_host: bool,
}

impl<'a> SendMedia<'a> {
    pub fn thread_id(self, thread_id: i64) -> Self {
        Self {
            thread_id: Some(thread_id),
            ..self
        }
    }

    pub fn thread_id_opt(self, thread_id: Option<i64>) -> Self {
        Self { thread_id, ..self }
    }

    pub fn text(self, text: Text<'a>) -> Self {
        Self {
            text: Some(text),
            ..self
        }
    }

    pub fn disable_notification(self) -> Self {
        Self {
            disable_notification: true,
            ..self
        }
    }

    pub fn disable_notification_bool(self, value: bool) -> Self {
        Self {
            disable_notification: value,
            ..self
        }
    }

    pub fn markup(self, markup: Markup<'a>) -> Self {
        Self {
            markup: Some(markup),
            ..self
        }
    }

    pub fn prefer_self_host(self) -> Self {
        Self {
            prefer_self_host: true,
            ..self
        }
    }

    pub async fn send(self) -> anyhow::Result<Response<ResultMessage>> {
        let mut body = json!(
            {
                "chat_id": self.chat.to_json(),
                "message_thread_id": self.thread_id,
                "disable_notification": self.disable_notification
            }
        );
        let (method, url, retry_multipart) = match &self.media {
            Media::Photo(photo) => {
                let url = photo.input.to_url(0);
                body["photo"] = url.clone().into();
                body["has_spoiler"] = photo.has_spoiler.into();
                ("sendPhoto", url, matches!(photo.input, MediaInput::Url(_)))
            }
            Media::Video(video) => {
                let url = video.input.to_url(0);
                body["video"] = url.clone().into();
                body["supports_streaming"] = true.into();
                body["has_spoiler"] = video.has_spoiler.into();
                ("sendVideo", url, matches!(video.input, MediaInput::Url(_)))
            }
            Media::Document(document) => {
                let url = document.input.to_url(0);
                body["document"] = url.clone().into();
                (
                    "sendDocument",
                    url,
                    matches!(document.input, MediaInput::Url(_)),
                )
            }
        };
        if let Some(text) = self.text {
            let (text, entities) = text.into_json();
            let body = body.as_object_mut().unwrap();
            body.insert("caption".into(), text);
            body.insert("caption_entities".into(), entities);
        }
        if let Some(markup) = self.markup {
            body["reply_markup"] = markup.into_json();
        }

        let mut resp = self
            .base
            .send_request(method, &body, [self.media.clone()], self.prefer_self_host)
            .await?;
        if retry_multipart && is_media_failure(&resp) {
            warn!("failed to send media with URL, retrying with HTTP multipart. url '{url}', description '{}'", resp.description.as_deref().unwrap_or("*no description*"));
            resp = self
                .base
                .send_request_force_download(method, &body, [self.media], self.prefer_self_host)
                .await?;
        }
        Ok(resp)
    }
}

pub struct SendMediaGroup<'a> {
    base: Request<'a>,
    chat: &'a ConfigChat,
    medias: Vec<Media<'a>>,
    thread_id: Option<i64>,
    text: Option<Text<'a>>,
    disable_notification: bool,
    prefer_self_host: bool,
}

impl<'a> SendMediaGroup<'a> {
    pub fn thread_id(self, thread_id: i64) -> Self {
        Self {
            thread_id: Some(thread_id),
            ..self
        }
    }

    pub fn thread_id_opt(self, thread_id: Option<i64>) -> Self {
        Self { thread_id, ..self }
    }

    pub fn media(mut self, new: Media<'a>) -> Self {
        self.medias.push(new);
        self
    }

    pub fn medias(mut self, new: impl IntoIterator<Item = Media<'a>>) -> Self {
        self.medias.append(&mut new.into_iter().collect());
        self
    }

    pub fn text(self, text: Text<'a>) -> Self {
        Self {
            text: Some(text),
            ..self
        }
    }

    pub fn disable_notification(self) -> Self {
        Self {
            disable_notification: true,
            ..self
        }
    }

    pub fn disable_notification_bool(self, value: bool) -> Self {
        Self {
            disable_notification: value,
            ..self
        }
    }

    pub fn prefer_self_host(self) -> Self {
        Self {
            prefer_self_host: true,
            ..self
        }
    }

    pub async fn send(mut self) -> anyhow::Result<Response<Vec<ResultMessage>>> {
        assert!(!self.medias.is_empty());

        let mut ret = vec![];

        if self.medias.len() > 10 {
            warn!(
                "media group size '{}' exceeds 10, splitting into multiple messages",
                self.medias.len()
            );
        }

        let mut medias = vec![];
        mem::swap(&mut self.medias, &mut medias);

        let mut iter = medias.chunks(10).peekable();
        while let Some(chunk) = iter.next() {
            let is_last_chunk = iter.peek().is_none();
            let text = is_last_chunk.then(|| self.text.take()).flatten();

            let resp = self.send_inner(chunk.iter().cloned(), text).await?;

            // If any chunk fails, return the response immediately
            if !resp.ok {
                return Ok(resp);
            }
            ret.append(&mut resp.result.unwrap());
        }

        Ok(Response {
            ok: true,
            description: None,
            result: Some(ret),
        })
    }

    async fn send_inner(
        &self,
        medias: impl IntoIterator<Item = Media<'a>>,
        text: Option<Text<'a>>,
    ) -> anyhow::Result<Response<Vec<ResultMessage>>> {
        let medias = medias.into_iter().collect_vec();

        let mut retry_multipart = false;
        let mut media = medias
            .iter()
            .enumerate()
            .map(|(i, media)| match media {
                Media::Photo(photo) => {
                    retry_multipart |= matches!(photo.input, MediaInput::Url(_));
                    json!({
                        "type": "photo",
                        "media": photo.input.to_url(i),
                        "has_spoiler": photo.has_spoiler,
                    })
                }
                Media::Video(video) => {
                    retry_multipart |= matches!(video.input, MediaInput::Url(_));
                    json!({
                        "type": "video",
                        "media": video.input.to_url(i),
                        "supports_streaming": true,
                        "has_spoiler": video.has_spoiler,
                    })
                }
                Media::Document(document) => {
                    retry_multipart |= matches!(document.input, MediaInput::Url(_));
                    json!({
                        "type": "document",
                        "media": document.input.to_url(i),
                    })
                }
            })
            .collect::<Vec<_>>();
        if let Some(text) = text {
            let (text, entities) = text.into_json();
            let first_media = media.first_mut().unwrap().as_object_mut().unwrap();
            first_media.insert("caption".into(), text);
            first_media.insert("caption_entities".into(), entities);
        }

        let body = json!(
            {
                "chat_id": self.chat.to_json(),
                "message_thread_id": self.thread_id,
                "media": media,
                "disable_notification": self.disable_notification
            }
        );

        let mut resp = self
            .base
            .send_request(
                "sendMediaGroup",
                &body,
                medias.clone(),
                self.prefer_self_host,
            )
            .await?;
        if retry_multipart && is_media_failure(&resp) {
            warn!("failed to send media group with URLs, retrying with HTTP multipart. urls '{medias:?}', description '{}'", resp.description.as_deref().unwrap_or("*no description*"));
            resp = self
                .base
                .send_request_force_download("sendMediaGroup", &body, medias, self.prefer_self_host)
                .await?;
        }
        Ok(resp)
    }
}

pub struct EditMessageText<'a> {
    base: Request<'a>,
    chat: &'a ConfigChat,
    message_id: i64,
    text: Text<'a>,
    link_preview: Option<LinkPreview<'a>>,
}

impl<'a> EditMessageText<'a> {
    pub fn link_preview(self, options: LinkPreview<'a>) -> Self {
        Self {
            link_preview: Some(options),
            ..self
        }
    }

    pub async fn send(self) -> anyhow::Result<Response<ResultMessage>> {
        let mut body = json!(
            {
                "chat_id": self.chat.to_json(),
                "message_id": self.message_id,
            }
        );
        let (text, entities) = self.text.into_json();
        {
            let body = body.as_object_mut().unwrap();
            body.insert("text".into(), text);
            body.insert("entities".into(), entities);
        }
        if let Some(link_preview) = self.link_preview {
            body["link_preview_options"] = link_preview.into_json();
        }
        self.base
            .send_request("editMessageText", &body, [], false)
            .await
    }
}

pub struct EditMessageCaption<'a> {
    base: Request<'a>,
    chat: &'a ConfigChat,
    message_id: i64,
    text: Option<Text<'a>>,
}

impl<'a> EditMessageCaption<'a> {
    pub fn text(self, text: Text<'a>) -> Self {
        Self {
            text: Some(text),
            ..self
        }
    }

    pub async fn send(self) -> anyhow::Result<Response<ResultMessage>> {
        let mut body = json!(
            {
                "chat_id": self.chat.to_json(),
                "message_id": self.message_id,
            }
        );
        if let Some(text) = self.text {
            let (text, entities) = text.into_json();
            let body = body.as_object_mut().unwrap();
            body.insert("caption".into(), text);
            body.insert("caption_entities".into(), entities);
        }
        self.base
            .send_request("editMessageCaption", &body, [], false)
            .await
    }
}

pub struct EditMessageMedia<'a> {
    base: Request<'a>,
    chat: &'a ConfigChat,
    message_id: i64,
    text: Option<Text<'a>>,
    media: Media<'a>,
    prefer_self_host: bool,
}

impl<'a> EditMessageMedia<'a> {
    pub fn text(self, text: Text<'a>) -> Self {
        Self {
            text: Some(text),
            ..self
        }
    }

    pub fn prefer_self_host(self) -> Self {
        Self {
            prefer_self_host: true,
            ..self
        }
    }

    pub async fn send(self) -> anyhow::Result<Response<ResultMessage>> {
        let retry_multipart;
        let mut body = json!({
            "chat_id": self.chat.to_json(),
            "message_id": self.message_id,
            "media": match &self.media {
                Media::Photo(photo) => {
                    retry_multipart = matches!(photo.input, MediaInput::Url(_));
                    json!({
                        "type": "photo",
                        "media": photo.input.to_url(0),
                        "has_spoiler": photo.has_spoiler,
                    })
                }
                Media::Video(video) => {
                    retry_multipart = matches!(video.input, MediaInput::Url(_));
                    json!({
                        "type": "video",
                        "media": video.input.to_url(0),
                        "width": video.resolution.map(|r| r.width),
                        "height": video.resolution.map(|r| r.height),
                        "supports_streaming": true,
                        "has_spoiler": video.has_spoiler,
                    })
                }
                Media::Document(document) => {
                    retry_multipart = matches!(document.input, MediaInput::Url(_));
                    json!({
                        "type": "document",
                        "media": document.input.to_url(0),
                    })
                }
            }
        });
        if let Some(text) = self.text {
            let (text, entities) = text.into_json();
            let media = body["media"].as_object_mut().unwrap();
            media.insert("caption".into(), text);
            media.insert("caption_entities".into(), entities);
        }

        let mut resp = self
            .base
            .send_request(
                "editMessageMedia",
                &body,
                [self.media.clone()],
                self.prefer_self_host,
            )
            .await?;
        if retry_multipart && is_media_failure(&resp) {
            warn!("failed to send media with URL, retrying with HTTP multipart. url '{:?}', description '{}'", self.media.input(), resp.description.as_deref().unwrap_or("*no description*"));
            resp = self
                .base
                .send_request_force_download(
                    "editMessageMedia",
                    &body,
                    [self.media],
                    self.prefer_self_host,
                )
                .await?;
        }
        Ok(resp)
    }
}

pub enum Entity<'a> {
    Link(&'a str),
    Quote,
}

pub struct Text<'a> {
    text: Cow<'a, str>,
    entities: Vec<(Range<usize>, Entity<'a>)>,
}

impl<'a> Text<'a> {
    pub fn new() -> Self {
        Self {
            text: Cow::Borrowed(""),
            entities: vec![],
        }
    }

    pub fn plain(text: impl Into<Cow<'a, str>>) -> Self {
        Self {
            text: text.into(),
            entities: vec![],
        }
    }

    pub fn link(text: impl Into<Cow<'a, str>>, link: &'a str) -> Self {
        let text = text.into();
        let entity = (0..text.encode_utf16().count(), Entity::Link(link));
        Self {
            text,
            entities: vec![entity],
        }
    }

    pub fn push_plain(&mut self, text: impl AsRef<str>) {
        self.text.to_mut().push_str(text.as_ref());
    }

    pub fn push_link(&mut self, text: impl AsRef<str>, link: &'a str) {
        let begin = self.text.encode_utf16().count();
        self.text.to_mut().push_str(text.as_ref());
        self.entities
            .push((begin..self.text.encode_utf16().count(), Entity::Link(link)));
    }

    pub fn push_quote(&mut self, content: impl FnOnce(&mut Self)) {
        let begin = self.text.encode_utf16().count();
        content(self);
        self.entities
            .push((begin..self.text.encode_utf16().count(), Entity::Quote));
    }

    pub fn push_content(&mut self, content: &'a PostContent) {
        content.parts().for_each(|part| match part {
            PostContentPart::Plain(text) => self.push_plain(text),
            PostContentPart::Link { display, url } => self.push_link(display, url),
            PostContentPart::InlineAttachment(_) => {
                // Ignore, we handle it in post.attachments
            }
        });
    }

    fn into_json(self) -> (json::Value, json::Value) {
        let entities = self
            .entities
            .into_iter()
            .map(|(range, entity)| match entity {
                Entity::Link(url) => json!({
                    "type": "text_link",
                    "offset": range.start,
                    "length": range.end - range.start,
                    "url": url,
                }),
                Entity::Quote => json!({
                    "type": "blockquote",
                    "offset": range.start,
                    "length": range.end - range.start,
                }),
            })
            .collect_vec();
        (
            json::Value::String(self.text.into()),
            json::Value::Array(entities),
        )
    }
}

#[derive(Deserialize)]
pub struct Response<R = IgnoredAny> {
    pub ok: bool,
    pub description: Option<String>,
    pub result: Option<R>,
}

impl<R> Response<R> {
    pub fn discard_result(self) -> Response<IgnoredAny> {
        Response {
            ok: self.ok,
            description: self.description,
            result: Some(IgnoredAny),
        }
    }
}

#[derive(Deserialize)]
pub struct ResultMessage {
    pub message_id: i64,
}

fn is_media_failure<T>(resp: &Response<T>) -> bool {
    if resp.ok || resp.description.is_none() {
        return false;
    }

    let description = resp.description.as_deref().unwrap();

    description.contains("WEBPAGE_") || // https://core.telegram.org/method/messages.sendMedia
    description.contains("failed to get HTTP URL content") ||
    description.contains("wrong file identifier/HTTP URL specified")
}

fn form_append_json(form: Form, obj: &json::Map<String, json::Value>) -> Form {
    obj.iter().fold(form, |form, (key, value)| {
        let value = match value {
            json::Value::Null => None,
            json::Value::Bool(value) => Some(value.to_string()),
            json::Value::Number(value) => Some(value.to_string()),
            json::Value::String(value) => Some(value.clone()),
            json::Value::Array(value) => Some(json::to_string(value).unwrap()),
            json::Value::Object(value) => Some(json::to_string(value).unwrap()),
        };
        if let Some(value) = value {
            form.text(key.clone(), value)
        } else {
            form
        }
    })
}

// Converts all Url files into Memory files
async fn download_files<'a>(
    files: impl IntoIterator<Item = Media<'a>>,
) -> anyhow::Result<Vec<Media<'a>>> {
    let mut ret = vec![];

    for mut file in files.into_iter() {
        if matches!(file.input(), MediaInput::Memory { .. }) {
            ret.push(file);
            continue;
        }

        let input = file.input_mut();

        let url = match &input {
            MediaInput::Url(url) => *url,
            MediaInput::Memory { .. } => unreachable!(),
        };

        trace!("downloading media from url '{url}'");

        let resp = helper::reqwest_client()?
            .get(url)
            .send()
            .await
            .map_err(|err| anyhow!("failed to download file: {err} from url '{url}'"))?;

        let status = resp.status();
        ensure!(
            status.is_success(),
            "response of downloading file is not success: {status} from url '{url}'"
        );

        let data = resp.bytes().await.map_err(|err| {
            let rustfmt_bug = "failed to obtain bytes for downloading file";
            anyhow!("{rustfmt_bug}: {err}, status: {status} from url '{url}'")
        })?;

        *input = MediaInput::Memory {
            data,
            filename: None,
        };

        // TODO: Replace failed image with a fallback image

        ret.push(file);
    }

    Ok(ret)
}

fn media_into_part(i: usize, bytes: Bytes, is_photo: bool) -> anyhow::Result<Part> {
    let part = if is_photo {
        let image_reader = image::ImageReader::new(Cursor::new(&bytes))
            .with_guessed_format()
            .map_err(|err| anyhow!("failed to guess format for downloaded image: {err}"))?;

        let format = image_reader.format();
        let mut image = image_reader
            .decode()
            .map_err(|err| anyhow!("failed to decode downloaded image: {err}"))?;
        let (width, height) = image.dimensions();

        // Based on my testing, the // actual limit is <=10001 :)
        const LIMIT: u32 = 10000;

        let total = width + height;
        if total > LIMIT {
            // Scaledown
            let ratio = (width as f64 / total as f64, height as f64 / total as f64);
            let new = (
                (LIMIT as f64 * ratio.0).floor() as u32,
                (LIMIT as f64 * ratio.1).floor() as u32,
            );
            warn!(
                "photo #{i} dimensions {width},{height} exceeds the limit, scale down to {},{}",
                new.0, new.1
            );

            image = image.resize(new.0, new.1, ImageFilterType::Lanczos3);

            let mut bytes: Vec<u8> = Vec::new();
            image.write_to(&mut Cursor::new(&mut bytes), format.unwrap())?;
            Part::stream(bytes)
        } else {
            Part::stream(bytes)
        }
    } else {
        Part::stream(bytes)
    }
    .file_name("");

    Ok(part)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_urls() {
        assert_eq!(
            make_api_url_impl(
                &Uri::from_static("https://api.telegram.org"),
                "TOKEN",
                "sendMessage"
            ),
            "https://api.telegram.org/botTOKEN/sendMessage"
        );
        assert_eq!(
            make_api_url_impl(
                &Uri::from_static("https://api.telegram.org/"),
                "TOKEN",
                "sendMessage"
            ),
            "https://api.telegram.org/botTOKEN/sendMessage"
        );
        assert_eq!(
            make_api_url_impl(
                &Uri::from_static("http://172.24.5.218:8081"),
                "TOKEN",
                "sendMessage"
            ),
            "http://172.24.5.218:8081/botTOKEN/sendMessage"
        );
    }
}
