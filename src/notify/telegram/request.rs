use std::{borrow::Cow, ops::Range};

use anyhow::anyhow;
use itertools::Itertools;
use serde::{
    de::{DeserializeOwned, IgnoredAny},
    Deserialize,
};
use serde_json::{self as json, json};

use super::ConfigChat;

pub struct Request<'a> {
    token: &'a str,
}

impl<'a> Request<'a> {
    pub fn new(token: &'a str) -> Self {
        Self { token }
    }

    async fn send_request<T: DeserializeOwned>(
        self,
        method: &str,
        body: json::Value,
    ) -> anyhow::Result<Response<T>> {
        let url = format!("https://api.telegram.org/bot{}/{}", self.token, method);
        let resp = reqwest::Client::new()
            .post(url)
            .json(&body)
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
        Ok(resp)
    }

    pub fn send_message(self, chat: &'a ConfigChat, text: Text<'a>) -> SendMessage<'a> {
        SendMessage {
            base: self,
            chat,
            text,
            thread_id: None,
            disable_notification: false,
            disable_link_preview: false,
            markup: None,
        }
    }

    pub fn send_photo(self, chat: &'a ConfigChat, photo: &'a str) -> SendMedia<'a> {
        SendMedia {
            base: self,
            chat,
            media: Media::Photo(photo),
            thread_id: None,
            text: None,
            disable_notification: false,
            markup: None,
        }
    }

    pub fn send_video(self, chat: &'a ConfigChat, video: &'a str) -> SendMedia<'a> {
        SendMedia {
            base: self,
            chat,
            media: Media::Photo(video),
            thread_id: None,
            text: None,
            disable_notification: false,
            markup: None,
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

impl<'a> Markup<'a> {
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

pub struct SendMessage<'a> {
    base: Request<'a>,
    chat: &'a ConfigChat,
    text: Text<'a>,
    thread_id: Option<i64>,
    disable_notification: bool,
    disable_link_preview: bool,
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

    pub fn disable_link_preview(self) -> Self {
        Self {
            disable_link_preview: true,
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
        if self.disable_link_preview {
            body["link_preview_options"] = json!({ "is_disabled": true });
        }
        if let Some(markup) = self.markup {
            body["reply_markup"] = markup.into_json();
        }
        self.base.send_request("sendMessage", body).await
    }
}

pub enum Media<'a> {
    Photo(&'a str), // TODO: Make a enum for it
    Video(&'a str),
}

pub struct SendMedia<'a> {
    base: Request<'a>,
    chat: &'a ConfigChat,
    media: Media<'a>,
    thread_id: Option<i64>,
    text: Option<Text<'a>>,
    disable_notification: bool,
    markup: Option<Markup<'a>>,
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

    pub async fn send(self) -> anyhow::Result<Response<ResultMessage>> {
        let mut body = json!(
            {
                "chat_id": self.chat.to_json(),
                "message_thread_id": self.thread_id,
                "disable_notification": self.disable_notification
            }
        );
        let method = match self.media {
            Media::Photo(url) => {
                body["photo"] = url.into();
                "sendPhoto"
            }
            Media::Video(url) => {
                body["video"] = url.into();
                "sendVideo"
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
        self.base.send_request(method, body).await
    }
}

pub struct SendMediaGroup<'a> {
    base: Request<'a>,
    chat: &'a ConfigChat,
    medias: Vec<Media<'a>>,
    thread_id: Option<i64>,
    text: Option<Text<'a>>,
    disable_notification: bool,
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

    pub async fn send(self) -> anyhow::Result<Response<Vec<ResultMessage>>> {
        assert!(!self.medias.is_empty());

        let mut media = self
            .medias
            .into_iter()
            .map(|media| match media {
                Media::Photo(url) => {
                    json!({
                        "type": "photo",
                        "media": url,
                    })
                }
                Media::Video(url) => {
                    json!({
                        "type": "video",
                        "media": url,
                    })
                }
            })
            .collect::<Vec<_>>();
        if let Some(text) = self.text {
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
        self.base.send_request("sendMediaGroup", body).await
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
        self.base.send_request("editMessageCaption", body).await
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
