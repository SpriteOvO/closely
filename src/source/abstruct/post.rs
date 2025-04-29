use std::{fmt, slice, vec};

use anyhow::ensure;
use chrono::{DateTime, Local};

#[derive(Debug, Eq, PartialEq, Hash)]
pub struct PostPlatformUniqueId(String);

#[derive(Clone, Debug, PartialEq)]
pub struct Post {
    pub user: Option<User>,
    pub content: PostContent,
    pub(in crate::source) urls: PostUrls,
    pub time: DateTime<Local>,
    pub is_pinned: bool,
    pub repost_from: Option<RepostFrom>,
    pub(in crate::source) attachments: Vec<PostAttachment>,
}

impl Post {
    pub fn platform_unique_id(&self) -> PostPlatformUniqueId {
        PostPlatformUniqueId(self.urls.major().unique_id().into())
    }

    pub fn attachments_recursive(&self, include_inlined: bool) -> Vec<&PostAttachment> {
        if let Some(RepostFrom::Recursion(repost_from)) = &self.repost_from {
            self.attachments
                .iter()
                .chain(self.content.parts().filter_map(|content| {
                    if !include_inlined {
                        return None;
                    }
                    if let PostContentPart::InlineAttachment(attachment) = content {
                        Some(attachment)
                    } else {
                        None
                    }
                }))
                .chain(repost_from.attachments_recursive(include_inlined))
                .collect()
        } else {
            self.attachments.iter().collect()
        }
    }

    pub fn urls_recursive(&self) -> PostUrlsRef {
        if let Some(RepostFrom::Recursion(repost_from)) = &self.repost_from {
            let mut v = self
                .urls
                .iter()
                .chain(repost_from.urls_recursive().into_iter().skip(1)) // Skip the major URL from the repost
                .collect::<Vec<_>>();
            v.dedup_by_key(|url| url.unique_id());
            PostUrlsRef(v)
        } else {
            PostUrlsRef(self.urls.iter().collect())
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PostUrls(Vec<PostUrl>);

impl PostUrls {
    pub fn new(url: PostUrl) -> Self {
        Self(vec![url])
    }

    pub fn from_iter(urls: impl IntoIterator<Item = PostUrl>) -> anyhow::Result<Self> {
        let urls = urls.into_iter().collect::<Vec<_>>();
        ensure!(!urls.is_empty(), "urls must not be empty");
        Ok(Self(urls))
    }

    pub fn major(&self) -> &PostUrl {
        self.0.first().unwrap()
    }

    pub fn iter(&self) -> slice::Iter<PostUrl> {
        self.0.iter()
    }
}

impl From<PostUrl> for PostUrls {
    fn from(url: PostUrl) -> Self {
        Self::new(url)
    }
}

#[derive(Debug)]
pub struct PostUrlsRef<'a>(Vec<&'a PostUrl>);

impl<'a> PostUrlsRef<'a> {
    pub fn major(&self) -> &'a PostUrl {
        self.0.first().unwrap()
    }

    pub fn iter(&self) -> slice::Iter<'_, &'a PostUrl> {
        self.0.iter()
    }

    pub fn into_iter(self) -> vec::IntoIter<&'a PostUrl> {
        self.0.into_iter()
    }
}

#[derive(Clone, Debug)]
pub enum PostUrl {
    Clickable(PostUrlClickable),
    // For some cases. a post doesn't have a URL (e.g. deleted post), but we still need something
    // unique to identify it
    Identity(String),
}

impl PostUrl {
    pub fn new_clickable(url: impl Into<String>, display: impl Into<String>) -> Self {
        PostUrl::Clickable(PostUrlClickable {
            url: url.into(),
            display: display.into(),
        })
    }

    pub fn as_clickable(&self) -> Option<&PostUrlClickable> {
        match self {
            PostUrl::Clickable(clickable) => Some(clickable),
            _ => None,
        }
    }

    pub fn unique_id(&self) -> &str {
        let id = match self {
            PostUrl::Clickable(clickable) => &clickable.url,
            PostUrl::Identity(identity) => identity,
        };
        assert!(!id.is_empty());
        id
    }
}

impl PartialEq for PostUrl {
    fn eq(&self, other: &Self) -> bool {
        self.unique_id().eq(other.unique_id())
    }
}

#[derive(Clone, Debug)]
pub struct PostUrlClickable {
    pub url: String,
    pub display: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PostContent(Vec<PostContentPart>);

impl PostContent {
    pub fn from_parts(parts: impl IntoIterator<Item = PostContentPart>) -> Self {
        Self(parts.into_iter().collect())
    }

    pub fn plain(text: impl Into<String>) -> Self {
        Self(vec![PostContentPart::Plain(text.into())])
    }

    pub fn fallback(&self) -> String {
        self.0
            .iter()
            .map(|part| match part {
                PostContentPart::Plain(text) => text.as_str(),
                PostContentPart::Link { url, .. } => url.as_str(),
                PostContentPart::InlineAttachment(attachment) => match attachment {
                    PostAttachment::Image(attachment) => attachment.media_url.as_str(),
                    PostAttachment::Video(attachment) => attachment.media_url.as_str(),
                },
            })
            .collect::<String>()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn parts(&self) -> impl Iterator<Item = &PostContentPart> {
        self.0.iter()
    }

    //

    pub fn push_content(&mut self, other: Self) {
        self.0.extend(other.0);
    }

    pub fn with_content(mut self, other: Self) -> Self {
        self.push_content(other);
        self
    }

    pub fn push_plain(&mut self, text: impl Into<String>) {
        self.0.push(PostContentPart::Plain(text.into()));
    }

    pub fn with_plain(mut self, text: impl Into<String>) -> Self {
        self.push_plain(text);
        self
    }

    pub fn push_link(&mut self, display: impl Into<String>, url: impl Into<String>) {
        self.0.push(PostContentPart::Link {
            display: display.into(),
            url: url.into(),
        });
    }

    pub fn with_link(mut self, display: impl Into<String>, url: impl Into<String>) -> Self {
        self.push_link(display, url);
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PostContentPart {
    Plain(String),
    Link { display: String, url: String },
    InlineAttachment(PostAttachment),
}

#[derive(Clone, Debug, PartialEq)]
pub struct User {
    pub nickname: String,
    pub profile_url: String,
    pub avatar_url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RepostFrom {
    Recursion(Box<Post>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum PostAttachment {
    Image(PostAttachmentImage),
    Video(PostAttachmentVideo),
}

#[derive(Clone, Debug)]
pub struct PostAttachmentImage {
    pub media_url: String,
    pub has_spoiler: bool,
}

impl PartialEq for PostAttachmentImage {
    fn eq(&self, other: &Self) -> bool {
        self.media_url.eq(&other.media_url)
    }
}

#[derive(Clone, Debug)]
pub struct PostAttachmentVideo {
    pub media_url: String,
    pub has_spoiler: bool,
}

impl PartialEq for PostAttachmentVideo {
    fn eq(&self, other: &Self) -> bool {
        self.media_url.eq(&other.media_url)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Posts(pub(in crate::source) Vec<Post>);

impl fmt::Display for Posts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?}",
            self.0.iter().map(|p| p.urls.major()).collect::<Vec<_>>()
        )
    }
}

#[derive(Clone, Debug)]
pub struct PostsRef<'a>(pub Vec<&'a Post>);

impl fmt::Display for PostsRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?}",
            self.0.iter().map(|p| p.urls.major()).collect::<Vec<_>>()
        )
    }
}
