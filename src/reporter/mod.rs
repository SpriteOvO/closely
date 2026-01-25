mod norec;

use std::{
    borrow::Cow,
    cmp::Ordering,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, UNIX_EPOCH},
};

use anyhow::anyhow;
use norec::NoRec;
use opentelemetry_otlp::WithExportConfig as _;
use regex::Regex;
use reqwest::Url;
use serde::Deserialize;
use spdlog::{
    Record, StringBuf,
    formatter::{FormatterContext, PatternFormatter, pattern},
    prelude::*,
    sink::{GetSinkProp, Sink, SinkProp},
};
use spdlog_opentelemetry::OpenTelemetrySink;

use crate::{
    cli,
    config::{self, Accessor, Config, Validator},
    notify,
    platform::PlatformMetadata,
    prop,
    source::{Notification, NotificationKind, StatusSource},
};

#[derive(Debug, PartialEq, Deserialize)]
pub struct ConfigReporterRaw {
    pub(crate) log: Accessor<Option<ConfigReporterLog>>,
    pub(crate) heartbeat: Accessor<Option<ConfigHeartbeat>>,
}

impl Validator for ConfigReporterRaw {
    fn validate(&self) -> anyhow::Result<()> {
        self.log.validate()?;
        self.heartbeat.validate()?;
        Ok(())
    }
}

impl ConfigReporterRaw {
    pub fn init(&self, notify_map: &config::NotifyMap) -> anyhow::Result<()> {
        if let Some(log) = &*self.log {
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
    pub(crate) opentelemetry: Option<ConfigReporterLogOpenTelemetry>,
    #[serde(rename = "notify")]
    pub(crate) notify_ref: Option<Vec<config::NotifyRef>>,
}

impl Validator for ConfigReporterLog {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(notify_ref) = &self.notify_ref {
            notify_ref
                .iter()
                .map(|notify_ref| Config::global().notify_map().get_by_ref(notify_ref))
                .collect::<Result<Vec<_>, _>>()?;
        }
        Ok(())
    }
}

impl ConfigReporterLog {
    pub fn init(&self, notify_map: &config::NotifyMap) -> anyhow::Result<()> {
        let mut sinks: Vec<Arc<dyn Sink>> = Vec::new();

        if let Some(otel) = &self.opentelemetry {
            sinks.push(Arc::new(
                OpenTelemetrySink::builder()
                    .provider(&otel.provider()?)
                    .build()?,
            ));
        }
        if let Some(notify_ref) = &self.notify_ref {
            let notify = notify_ref
                .iter()
                .map(|notify_ref| notify_map.get_by_ref(notify_ref).unwrap())
                .collect::<Vec<_>>();
            sinks.push(Arc::new(NotifySink::new(notify)));
        }

        let logger = spdlog::default_logger().fork_with(|logger| {
            logger.sinks_mut().append(&mut sinks);
            Ok(())
        })?;
        spdlog::set_default_logger(logger);

        Ok(())
    }
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct ConfigReporterLogOpenTelemetry {
    pub(crate) endpoint: Url,
}

impl ConfigReporterLogOpenTelemetry {
    fn provider(&self) -> anyhow::Result<opentelemetry_sdk::logs::SdkLoggerProvider> {
        let exporter = opentelemetry_otlp::LogExporter::builder()
            .with_tonic()
            .with_endpoint(self.endpoint.to_string())
            .build()
            .map_err(|err| anyhow!("failed to build opentelemetry_otlp::LogExporter: {err}"))?;
        let logger_provider = opentelemetry_sdk::logs::SdkLoggerProvider::builder()
            .with_resource(
                opentelemetry_sdk::Resource::builder()
                    .with_service_name(prop::PACKAGE.name)
                    .with_attribute(opentelemetry::KeyValue::new("service.version", cli::VER))
                    .build(),
            )
            .with_batch_exporter(exporter)
            .build();
        Ok(logger_provider)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigHeartbeat {
    #[serde(flatten)]
    pub kind: ConfigHeartbeatKind,
    #[serde(with = "humantime_serde")]
    pub interval: Duration,
}

impl Validator for ConfigHeartbeat {
    fn validate(&self) -> anyhow::Result<()> {
        match &self.kind {
            ConfigHeartbeatKind::HttpGet(http_get) => _ = Url::parse(&http_get.url)?,
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum ConfigHeartbeatKind {
    HttpGet(ConfigHeartbeatHttpGet),
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigHeartbeatHttpGet {
    pub(crate) url: String,
}

impl ConfigHeartbeatHttpGet {
    pub fn url(&self) -> Url {
        Url::parse(&self.url).unwrap()
    }
}

pub struct ReporterParams {
    pub heartbeat: Accessor<Option<ConfigHeartbeat>>,
}

// TODO: Make it configurable
const LOG_LEVEL_FILTER: LevelFilter = LevelFilter::MoreSevereEqual(Level::Warn);

struct NotifySink {
    prop: SinkProp,
    rt: tokio::runtime::Handle,
    notifiers: Vec<Box<dyn notify::NotifierTrait>>,
    no_rec: NoRec,
    hourly_errs: Mutex<(u64, usize)>,
}

impl NotifySink {
    const STATUS_SOURCE: StatusSource = StatusSource {
        platform: PlatformMetadata {
            display_name: "Closely",
        },
        user: None,
    };

    fn new(notify: Vec<Accessor<notify::NotifierConfig>>) -> Self {
        let prop = SinkProp::default();
        prop.set_level_filter(LevelFilter::MoreSevereEqual(Level::Warn));
        prop.set_formatter(PatternFormatter::new(pattern!(
            "#log #{level} {payload}{eol}@{source}{eol}{kv}"
        )));
        Self {
            prop,
            rt: tokio::runtime::Handle::current(),
            notifiers: notify.into_iter().map(notify::notifier).collect(),
            no_rec: NoRec::new(),
            hourly_errs: Mutex::new((0, 0)),
        }
    }

    fn notify_log(&self, message: impl Into<String>) {
        let notification = Notification {
            kind: NotificationKind::Log(message.into()),
            source: &Self::STATUS_SOURCE,
        };

        tokio::task::block_in_place(|| {
            for notifier in &self.notifiers {
                self.rt
                    .block_on(async { notify::notify(&**notifier, &notification).await });
            }
        });
    }

    fn should_skip(&self, record: &Record) -> bool {
        const ERR_LIMIT: usize = 20;

        let this_hour = record.time().duration_since(UNIX_EPOCH).unwrap().as_secs() / 3600;

        let mut hourly_errs = self.hourly_errs.lock().unwrap();
        if hourly_errs.0 != this_hour {
            if hourly_errs.1 > ERR_LIMIT {
                self.notify_log(format!(
                    "⚠️ {} errors were skipped prior to this, resumed error notifications",
                    hourly_errs.1 - ERR_LIMIT
                ));
            }
            *hourly_errs = (this_hour, 0);
        }

        if LevelFilter::MoreSevereEqual(Level::Error).test(record.level()) {
            hourly_errs.1 += 1;

            match hourly_errs.1.cmp(&(ERR_LIMIT + 1)) {
                Ordering::Equal => {
                    self.notify_log(format!(
                        "⚠️ too many errors (limit {ERR_LIMIT}), skip notifying in this hour. please check the logs through other means."
                    ));
                    true
                }
                Ordering::Greater => true,
                Ordering::Less => false,
            }
        } else {
            false
        }
    }

    // Workaround: https://github.com/seanmonstar/reqwest/issues/2365
    fn strip_url(buf: &str) -> Cow<'_, str> {
        static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" for url \(.+\)").unwrap());
        RE.replace_all(buf, r" for url (<stripped>)")
    }
}

impl GetSinkProp for NotifySink {
    fn prop(&self) -> &SinkProp {
        &self.prop
    }
}

impl Sink for NotifySink {
    fn log(&self, record: &Record) -> spdlog::Result<()> {
        let guard = self.no_rec.enter();
        if guard.is_none() {
            return Ok(());
        }
        if self.should_skip(record) {
            return Ok(());
        }

        let mut buf = StringBuf::new();
        let mut ctx = FormatterContext::new();
        self.prop.formatter().format(record, &mut buf, &mut ctx)?;

        self.notify_log(Self::strip_url(&buf));
        Ok(())
    }

    fn flush(&self) -> spdlog::Result<()> {
        Ok(()) // No-op
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_url_valid() {
        assert_eq!(
            NotifySink::strip_url(
                r"failed to sent request: error sending request for url (https://example.com/endpoint?a=%7Babc&_#)",
            ),
            r"failed to sent request: error sending request for url (<stripped>)"
        );
    }
}
