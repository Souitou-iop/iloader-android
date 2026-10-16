use serde::{Deserialize, Serialize};
use std::sync::{Arc, Once};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, Registry, fmt, registry::LookupSpan};

use crate::{CoreEvent, Host};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogRecord {
    pub level: u8,
    pub message: String,
    pub target: Option<String>,
    pub timestamp: String,
}

/// Forwards every tracing event to the host as `CoreEvent::Log`.
pub struct HostLoggingLayer {
    host: Arc<dyn Host>,
}

impl HostLoggingLayer {
    pub fn new(host: Arc<dyn Host>) -> Self {
        Self { host }
    }
}

impl<S> Layer<S> for HostLoggingLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        use tracing::field::Visit;

        let metadata = event.metadata();
        let level = match *metadata.level() {
            tracing::Level::TRACE => 1u8,
            tracing::Level::DEBUG => 2u8,
            tracing::Level::INFO => 3u8,
            tracing::Level::WARN => 4u8,
            tracing::Level::ERROR => 5u8,
        };

        let target = metadata.target().to_string();

        struct EventVisitor {
            message: String,
            fields: Vec<String>,
        }

        impl EventVisitor {
            fn push_field(&mut self, field: &tracing::field::Field, value: String) {
                if field.name() == "message" {
                    self.message = value;
                } else {
                    self.fields.push(format!("{}={}", field.name(), value));
                }
            }
        }

        impl Visit for EventVisitor {
            fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
                self.push_field(field, value.to_string());
            }

            fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
                self.push_field(field, value.to_string());
            }

            fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
                self.push_field(field, value.to_string());
            }

            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                self.push_field(field, value.to_string());
            }

            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                self.push_field(field, format!("{:?}", value));
            }
        }

        let mut visitor = EventVisitor {
            message: String::new(),
            fields: Vec::new(),
        };
        event.record(&mut visitor);

        let message = if visitor.fields.is_empty() {
            visitor.message
        } else {
            format!("{} ({})", visitor.message, visitor.fields.join(", "))
        };

        let timestamp = chrono::Local::now()
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string();

        let record = LogRecord {
            level,
            message,
            target: Some(target),
            timestamp,
        };

        self.host.emit(CoreEvent::Log(record));
    }
}

static INIT: Once = Once::new();

/// Sets up logging to `<data_dir>/logs` and to the host, and captures
/// panics in the log. Does nothing after the first call.
pub fn init(host: &Arc<dyn Host>) {
    INIT.call_once(|| {
        let log_dir = host.data_dir().join("logs");
        std::fs::create_dir_all(&log_dir).ok();

        let file_appender = tracing_appender::rolling::RollingFileAppender::builder()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("iloader")
            .filename_suffix("log")
            .max_log_files(2)
            .build(&log_dir)
            .expect("failed to create log file appender");

        let file_layer = fmt::layer()
            .with_writer(file_appender)
            .with_target(true)
            .with_ansi(false)
            .with_filter(tracing_subscriber::filter::LevelFilter::DEBUG);

        let host_layer = HostLoggingLayer::new(host.clone())
            .with_filter(tracing_subscriber::filter::LevelFilter::DEBUG);

        // A host may have set up its own subscriber already.
        if Registry::default()
            .with(file_layer)
            .with(host_layer)
            .try_init()
            .is_err()
        {
            return;
        }

        std::panic::set_hook(Box::new(|panic_info| {
            let thread = std::thread::current();
            let thread_name = thread.name().unwrap_or("<unnamed>");

            let message = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "<non-string panic payload>".to_string()
            };

            let location = panic_info
                .location()
                .map(|loc| format!("{}:{}", loc.file(), loc.line()))
                .unwrap_or_else(|| "<unknown>".to_string());

            let backtrace = std::backtrace::Backtrace::capture();

            tracing::error!(
                target: "panic",
                thread = thread_name,
                location = location,
                message = message,
                backtrace = %backtrace,
                "panic captured"
            );
        }));
    });
}
