use std::fmt;

use super::{DocumentRef, LiveStatus, PlaybackRef, PostsRef, StatusSource};

#[derive(Debug)]
pub struct Notification<'a> {
    pub kind: NotificationKind<'a>,
    pub source: &'a StatusSource,
}

impl fmt::Display for Notification<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)
    }
}

#[derive(Debug)]
pub enum NotificationKind<'a> {
    LiveOnline(&'a LiveStatus),
    LiveTitle(&'a LiveStatus, &'a str /* old title */),
    Posts(PostsRef<'a>),
    Log(String),
    Playback(PlaybackRef<'a>),
    Document(DocumentRef<'a>),
}

impl fmt::Display for NotificationKind<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LiveOnline(live_status) => write!(f, "{live_status}"),
            Self::LiveTitle(live_status, old_title) => {
                write!(f, "{live_status}, old title '{old_title}'")
            }
            Self::Posts(posts) => write!(f, "{posts}"),
            Self::Log(message) => write!(f, "log '{message}'"),
            Self::Playback(playback) => write!(f, "{playback}"),
            Self::Document(document) => write!(f, "{document}"),
        }
    }
}
