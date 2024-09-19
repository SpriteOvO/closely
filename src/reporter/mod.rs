mod norec;

use std::{sync::Arc, time::Duration};

use norec::NoRec;
use reqwest::Url;
use serde::Deserialize;
use spdlog::{
    formatter::{pattern, Formatter, PatternFormatter},
    prelude::*,
    sink::Sink,
    Record, StringBuf,
};

use crate::{
    config, notify,
    platform::PlatformMetadata,
    source::{Notification, NotificationKind, StatusSource},
};

#[derive(Debug, PartialEq, Deserialize)]
pub struct ConfigReporterRaw {
    pub(crate) log: Option<ConfigReporterLog>,
    pub(crate) heartbeat: Option<ConfigHeartbeat>,
}

impl ConfigReporterRaw {
    pub fn validate(&self, notify_map: &config::NotifyMap) -> anyhow::Result<()> {
        if let Some(log) = &self.log {
            log.validate(notify_map)?;
        }
        if let Some(heartbeat) = &self.heartbeat {
            heartbeat.validate()?;
        }
        Ok(())
    }

    pub fn init(&self, notify_map: &config::NotifyMap) -> anyhow::Result<()> {
        if let Some(log) = &self.log {
            log.init(notify_map)?;
        }
        Ok(())
    }

    pub fn reporter(&self) -> ReporterParams {
        ReporterParams {
            heartbeat: self.heartbeat.clone(),
        }
    }
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct ConfigReporterLog {
    #[serde(rename = "notify")]
    pub(crate) notify_ref: Vec<config::NotifyRef>,
}

impl ConfigReporterLog {
    pub fn validate(&self, notify_map: &config::NotifyMap) -> anyhow::Result<()> {
        self.notify_ref
            .iter()
            .map(|notify_ref| notify_map.get_by_ref(notify_ref))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(())
    }

    pub fn init(&self, notify_map: &config::NotifyMap) -> anyhow::Result<()> {
        let notify = self
            .notify_ref
            .iter()
            .map(|notify_ref| notify_map.get_by_ref(notify_ref).unwrap())
            .collect::<Vec<_>>();

        let sink = Arc::new(TelegramNotifySink::new(notify));
        let logger = spdlog::default_logger().fork_with(|logger| {
            logger.sinks_mut().push(sink);
            Ok(())
        })?;
        spdlog::set_default_logger(logger);

        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigHeartbeat {
    #[serde(flatten)]
    pub kind: ConfigHeartbeatKind,
    #[serde(with = "humantime_serde")]
    pub interval: Duration,
}

impl ConfigHeartbeat {
    pub fn validate(&self) -> anyhow::Result<()> {
        self.kind.validate()?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum ConfigHeartbeatKind {
    HttpGet(ConfigHeartbeatHttpGet),
}

impl ConfigHeartbeatKind {
    pub fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::HttpGet(http_get) => http_get.validate(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigHeartbeatHttpGet {
    pub(crate) url: String,
}

impl ConfigHeartbeatHttpGet {
    pub fn validate(&self) -> anyhow::Result<()> {
        Url::parse(&self.url)?;
        Ok(())
    }

    pub fn url(&self) -> Url {
        Url::parse(&self.url).unwrap()
    }
}

pub struct ReporterParams {
    pub heartbeat: Option<ConfigHeartbeat>,
}

// TODO: Make it configurable
const LOG_LEVEL_FILTER: LevelFilter = LevelFilter::MoreSevereEqual(Level::Warn);

struct TelegramNotifySink {
    rt: tokio::runtime::Handle,
    source: StatusSource,
    formatter: Box<dyn Formatter>,
    notifiers: Vec<Box<dyn notify::NotifierTrait>>,
    no_rec: NoRec,
}

impl TelegramNotifySink {
    fn new(notify: Vec<notify::platform::Config>) -> Self {
        Self {
            rt: tokio::runtime::Handle::current(),
            source: StatusSource {
                platform: PlatformMetadata {
                    display_name: "Closely",
                },
                user: None,
            },
            formatter: Box::new(PatternFormatter::new(pattern!(
                "#log #{level} {payload}{eol}@{source}"
            ))),
            notifiers: notify.into_iter().map(notify::notifier).collect(),
            no_rec: NoRec::new(),
        }
    }
}

impl Sink for TelegramNotifySink {
    fn log(&self, record: &Record) -> spdlog::Result<()> {
        if !self.should_log(record.level()) {
            return Ok(());
        }

        let guard = self.no_rec.enter();
        if guard.is_none() {
            return Ok(());
        }

        let mut buf = StringBuf::new();
        let _extra_info = self.formatter.format(record, &mut buf)?;

        let notification = Notification {
            kind: NotificationKind::Log(buf),
            source: &self.source,
        };

        tokio::task::block_in_place(|| {
            for notifier in &self.notifiers {
                self.rt
                    .block_on(async { notify::notify(&**notifier, &notification).await });
            }
        });

        Ok(())
    }

    fn flush(&self) -> spdlog::Result<()> {
        Ok(()) // No-op
    }

    fn level_filter(&self) -> LevelFilter {
        LOG_LEVEL_FILTER
    }

    fn set_level_filter(&self, _level_filter: LevelFilter) {
        unreachable!("no-op")
    }

    fn set_formatter(&self, _formatter: Box<dyn Formatter>) {
        unreachable!("no-op")
    }

    fn set_error_handler(&self, _handler: Option<spdlog::ErrorHandler>) {
        unreachable!("no-op")
    }
}
