use std::{fmt, time::SystemTime};

use humantime_serde::re::humantime;

#[derive(Clone, Debug, PartialEq)]
pub struct LiveStatus {
    pub kind: LiveStatusKind,
    pub title: String,
    pub streamer_name: String,
    pub cover_image_url: String,
    pub live_url: String,
}

impl fmt::Display for LiveStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "'{}' {}", self.streamer_name, self.kind)?;
        if let LiveStatusKind::Online { start_time } = self.kind {
            write!(
                f,
                " with title '{}' started at {:?}",
                self.title,
                start_time.map(humantime::format_rfc3339)
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LiveStatusKind {
    Online { start_time: Option<SystemTime> },
    Offline,
    Banned,
}

impl fmt::Display for LiveStatusKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Online { start_time: _ } => write!(f, "online"),
            Self::Offline => write!(f, "offline"),
            Self::Banned => write!(f, "banned"),
        }
    }
}
