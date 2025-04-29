use std::{collections::HashMap, fmt, vec};

use spdlog::prelude::*;
use tokio::sync::Mutex;

use super::{
    Document, DocumentRef, FileRef, Notification, NotificationKind, Playback, PlaybackRef,
    StatusSource,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Update {
    kind: UpdateKind,
    source: StatusSource, // TODO: rename the type
}

impl Update {
    pub fn new(kind: UpdateKind, source: StatusSource) -> Self {
        Self { kind, source }
    }

    pub async fn generate_notifications(&self) -> Vec<Notification<'_>> {
        match &self.kind {
            UpdateKind::Playback(playback) => {
                vec![Notification {
                    kind: NotificationKind::Playback(PlaybackRef {
                        live_start_time: playback.live_start_time,
                        local_file: (&playback.file_path, playback.format),
                        loaded: Mutex::new(HashMap::new()),
                    }),
                    source: &self.source,
                }]
            }
            UpdateKind::Document(document) => {
                let Ok(file) = FileRef::new(&document.file_path).await.inspect_err(|err| {
                    error!(
                        "failed to read document file '{:?}': {err}",
                        document.file_path
                    )
                }) else {
                    return vec![];
                };

                vec![Notification {
                    kind: NotificationKind::Document(DocumentRef { file }),
                    source: &self.source,
                }]
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateKind {
    Playback(Playback),
    Document(Document),
}

impl fmt::Display for UpdateKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Playback(playback) => write!(f, "{playback}"),
            Self::Document(document) => write!(f, "{document}"),
        }
    }
}
