//! Logging initialization module
//!
//! Provides production-ready logging with:
//! - UTC timestamps (RFC 3339 format)
//! - RUST_LOG environment variable support
//! - JSON output format option
//! - File output with rotation

use std::io;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter, Layer};

use crate::config::{Config, LogFormat, LogRotation};

/// Guard that must be held for the duration of the application
/// to ensure logs are flushed on shutdown
pub struct LoggingGuard {
    _console_guard: Option<WorkerGuard>,
    _file_guard: Option<WorkerGuard>,
}

/// Initialize the logging system based on configuration
///
/// Returns a guard that must be held until application shutdown
/// to ensure all logs are properly flushed.
pub fn init_logging(cfg: &Config) -> LoggingGuard {
    // Build env filter with RUST_LOG support
    let default_level = if cfg.browser_debug { "debug" } else { "info" };
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_level));

    // Time format for timestamps (RFC 3339)
    let timer = UtcTime::rfc_3339();

    // Console layer (always enabled)
    let (console_writer, console_guard) = tracing_appender::non_blocking(io::stdout());

    let console_layer = match cfg.log_format {
        LogFormat::Json => fmt::layer()
            .json()
            .with_timer(timer.clone())
            .with_target(true)
            .with_writer(console_writer)
            .boxed(),
        LogFormat::Text => fmt::layer()
            .with_timer(timer.clone())
            .with_target(true)
            .with_ansi(true)
            .with_writer(console_writer)
            .boxed(),
    };

    // File layer (optional)
    let (file_layer, file_guard) = if let Some(ref filename) = cfg.log_file {
        let rotation = match cfg.log_rotation {
            LogRotation::Hourly => Rotation::HOURLY,
            LogRotation::Daily => Rotation::DAILY,
            LogRotation::Never => Rotation::NEVER,
        };

        let file_appender = RollingFileAppender::new(rotation, &cfg.log_dir, filename);
        let (file_writer, guard) = tracing_appender::non_blocking(file_appender);

        let layer = match cfg.log_format {
            LogFormat::Json => fmt::layer()
                .json()
                .with_timer(timer)
                .with_target(true)
                .with_ansi(false)
                .with_writer(file_writer)
                .boxed(),
            LogFormat::Text => fmt::layer()
                .with_timer(timer)
                .with_target(true)
                .with_ansi(false)
                .with_writer(file_writer)
                .boxed(),
        };

        (Some(layer), Some(guard))
    } else {
        (None, None)
    };

    // Compose and initialize
    tracing_subscriber::registry()
        .with(env_filter)
        .with(console_layer)
        .with(file_layer)
        .init();

    LoggingGuard {
        _console_guard: Some(console_guard),
        _file_guard: file_guard,
    }
}
