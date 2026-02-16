use std::env;
use std::path::PathBuf;
use std::time::Duration;

use tracing::warn;

/// Log output format
#[derive(Debug, Clone, Default)]
pub enum LogFormat {
    #[default]
    Text,
    Json,
}

impl LogFormat {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "json" => Self::Json,
            _ => Self::Text,
        }
    }
}

/// Log file rotation strategy
#[derive(Debug, Clone, Default)]
pub enum LogRotation {
    #[default]
    Daily,
    Hourly,
    Never,
}

impl LogRotation {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "hourly" => Self::Hourly,
            "never" => Self::Never,
            _ => Self::Daily,
        }
    }
}

/// Application configuration loaded from environment variables
#[derive(Debug, Clone)]
pub struct Config {
    // Server settings
    pub grpc_port: String,
    pub http_port: String,

    // Auth credentials
    pub user_name: String,
    pub comp_id: String,
    pub user_pass: String,

    // Browser settings
    pub browser_headless: bool,
    pub browser_timeout: Duration,
    pub browser_debug: bool,

    // Job timeout settings
    pub vehicle_job_timeout: Duration,

    // Database
    pub sqlite_path: String,

    // Session settings
    pub session_ttl: Duration,
    pub cookie_ttl: Duration,

    // rust-logi API settings
    pub rust_logi_url: String,
    pub rust_logi_organization_id: String,

    // Logging settings
    pub log_format: LogFormat,
    pub log_file: Option<String>,
    pub log_dir: String,
    pub log_rotation: LogRotation,
}

impl Config {
    /// Load configuration from environment variables
    pub fn load() -> Self {
        // Try to load .env file
        load_env_file();

        let cfg = Config {
            grpc_port: get_env("GRPC_PORT", "50051"),
            http_port: get_env("HTTP_PORT", "8080"),
            user_name: get_env("USER_NAME", ""),
            comp_id: get_env("COMP_ID", ""),
            user_pass: get_env("USER_PASS", ""),
            browser_headless: get_env_bool("BROWSER_HEADLESS", true),
            browser_timeout: get_env_duration("BROWSER_TIMEOUT", Duration::from_secs(60)),
            browser_debug: get_env_bool("BROWSER_DEBUG", false),
            vehicle_job_timeout: get_env_duration("VEHICLE_JOB_TIMEOUT", Duration::from_secs(240)),
            sqlite_path: get_env("SQLITE_PATH", "./data/browser_render.db"),
            session_ttl: get_env_duration("SESSION_TTL", Duration::from_secs(600)), // 10 minutes
            cookie_ttl: get_env_duration("COOKIE_TTL", Duration::from_secs(86400)), // 24 hours
            rust_logi_url: get_env("RUST_LOGI_URL", ""),
            rust_logi_organization_id: get_env("RUST_LOGI_ORGANIZATION_ID", ""),
            log_format: LogFormat::from_str(&get_env("LOG_FORMAT", "text")),
            log_file: env::var("LOG_FILE").ok(),
            log_dir: get_env("LOG_DIR", "./logs"),
            log_rotation: LogRotation::from_str(&get_env("LOG_ROTATION", "daily")),
        };

        // Validate required fields
        if cfg.user_name.is_empty() || cfg.comp_id.is_empty() || cfg.user_pass.is_empty() {
            warn!("Authentication credentials not set in environment variables");
        }

        // Validate rust-logi settings
        if cfg.rust_logi_url.is_empty() {
            warn!("RUST_LOGI_URL not set - data will not be sent to rust-logi");
        }
        if cfg.rust_logi_organization_id.is_empty() {
            warn!("RUST_LOGI_ORGANIZATION_ID not set - required for rust-logi");
        }

        cfg
    }
}

/// Try to load .env file from multiple possible locations
fn load_env_file() {
    let possible_paths = [
        PathBuf::from(".env"),
        PathBuf::from("../.env"),
        env::current_dir()
            .map(|d| d.join(".env"))
            .unwrap_or_else(|_| PathBuf::from(".env")),
    ];

    for path in &possible_paths {
        if path.exists() {
            match dotenvy::from_path(path) {
                Ok(_) => {
                    tracing::info!("Loaded environment variables from {:?}", path);
                    break;
                }
                Err(e) => {
                    warn!("Error loading .env file from {:?}: {}", path, e);
                }
            }
        }
    }
}

/// Get string environment variable with default
fn get_env(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Get boolean environment variable with default
fn get_env_bool(key: &str, default: bool) -> bool {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Get duration environment variable with default
/// Supports both duration strings (e.g., "60s", "10m") and milliseconds as integer
fn get_env_duration(key: &str, default: Duration) -> Duration {
    let value = match env::var(key) {
        Ok(v) => v,
        Err(_) => return default,
    };

    // Try parsing as duration string (e.g., "60s", "10m", "1h")
    if let Some(duration) = parse_duration_string(&value) {
        return duration;
    }

    // Try parsing as milliseconds
    if let Ok(ms) = value.parse::<u64>() {
        return Duration::from_millis(ms);
    }

    default
}

/// Parse duration string like "60s", "10m", "1h"
fn parse_duration_string(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let (num_str, unit) = if s.ends_with("ms") {
        (&s[..s.len() - 2], "ms")
    } else if s.ends_with('s') {
        (&s[..s.len() - 1], "s")
    } else if s.ends_with('m') {
        (&s[..s.len() - 1], "m")
    } else if s.ends_with('h') {
        (&s[..s.len() - 1], "h")
    } else {
        return None;
    };

    let num: u64 = num_str.parse().ok()?;

    Some(match unit {
        "ms" => Duration::from_millis(num),
        "s" => Duration::from_secs(num),
        "m" => Duration::from_secs(num * 60),
        "h" => Duration::from_secs(num * 3600),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_string() {
        assert_eq!(
            parse_duration_string("60s"),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            parse_duration_string("10m"),
            Some(Duration::from_secs(600))
        );
        assert_eq!(
            parse_duration_string("1h"),
            Some(Duration::from_secs(3600))
        );
        assert_eq!(
            parse_duration_string("500ms"),
            Some(Duration::from_millis(500))
        );
        assert_eq!(parse_duration_string("invalid"), None);
    }
}
