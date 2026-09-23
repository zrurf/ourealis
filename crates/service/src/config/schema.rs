//! Configuration sections.
//!
//! The field names and nested section names are the TOML keys, so the manual in
//! `dev-notes/Ourealis 服务与Web实施文档.md` §3 is generated from this file's
//! shape rather than maintained next to it. Every field carries a default and is
//! serialised on `--print-config`, which is what `config::unknown_keys` compares a
//! file against.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::defaults;
use crate::error::{Result, ServiceError};

/// Everything the service reads from its configuration file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Layout version of the file itself.
    pub config_version: u32,
    /// Logging.
    pub log: LogConfig,
    /// Listeners and the facade switches.
    pub server: ServerConfig,
    /// gRPC facade limits.
    pub rpc: RpcConfig,
    /// HTTP facade behaviour.
    pub http: HttpConfig,
    /// Simulation execution.
    pub simulation: SimulationConfigSection,
    /// Where maps and results live.
    pub storage: StorageConfig,
    /// Static page behaviour.
    pub web: WebConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: crate::config::CONFIG_VERSION,
            log: LogConfig::default(),
            server: ServerConfig::default(),
            rpc: RpcConfig::default(),
            http: HttpConfig::default(),
            simulation: SimulationConfigSection::default(),
            storage: StorageConfig::default(),
            web: WebConfig::default(),
        }
    }
}

/// Log output shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Human-readable, one line per event.
    Pretty,
    /// One line per event with the fields inline.
    Compact,
    /// One JSON object per line.
    Json,
}

/// Logging.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    /// Maximum level: `trace`, `debug`, `info`, `warn` or `error`. `RUST_LOG`
    /// overrides it when set.
    pub level: String,
    /// Output shape.
    pub format: LogFormat,
    /// Colour output; turned off automatically when the output is not a terminal.
    pub ansi: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: defaults::LOG_LEVEL.to_string(),
            format: LogFormat::Pretty,
            ansi: true,
        }
    }
}

impl LogConfig {
    /// Rejects an unusable level or format.
    pub fn validate(&self) -> Result<()> {
        const LEVELS: [&str; 5] = ["trace", "debug", "info", "warn", "error"];
        // A directive list such as `info,ourealis=debug` is accepted as well.
        let first = self.level.split(',').next().unwrap_or("").trim();
        let first = first.split('=').next_back().unwrap_or(first).trim();
        if !LEVELS.contains(&first) {
            return Err(ServiceError::Config(format!(
                "log.level must start with one of {LEVELS:?}, got {:?}",
                self.level
            )));
        }
        Ok(())
    }
}

/// Listeners and facade switches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Serve the gRPC facade.
    pub rpc_enabled: bool,
    /// Address the gRPC facade listens on; port 0 lets the system choose.
    pub rpc_listen: String,
    /// Serve the HTTP facade, which carries REST, WebSocket and SSE together.
    pub http_enabled: bool,
    /// Address the HTTP facade listens on; port 0 lets the system choose.
    pub http_listen: String,
    /// Serve the embedded single-page application; requires `http_enabled`.
    pub web_enabled: bool,
    /// Seconds to wait for in-flight work during shutdown.
    pub shutdown_grace_s: f64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            rpc_enabled: true,
            rpc_listen: defaults::RPC_LISTEN.to_string(),
            http_enabled: true,
            http_listen: defaults::HTTP_LISTEN.to_string(),
            web_enabled: true,
            shutdown_grace_s: defaults::SHUTDOWN_GRACE_S,
        }
    }
}

impl ServerConfig {
    /// Rejects unusable addresses and durations.
    pub fn validate(&self) -> Result<()> {
        if self.rpc_enabled {
            validate_listen("server.rpc_listen", &self.rpc_listen)?;
        }
        if self.http_enabled {
            validate_listen("server.http_listen", &self.http_listen)?;
        }
        if !(0.0..=600.0).contains(&self.shutdown_grace_s) {
            return Err(ServiceError::Config(
                "server.shutdown_grace_s must be between 0 and 600 seconds".to_string(),
            ));
        }
        Ok(())
    }
}

/// A `host:port` listener address.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RpcConfig {
    /// Largest message the gRPC facade accepts or sends, in bytes.
    pub max_message_bytes: usize,
    /// Largest number of concurrent streams one connection may open.
    pub max_concurrent_streams: u32,
}

impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            max_message_bytes: defaults::RPC_MAX_MESSAGE_BYTES,
            max_concurrent_streams: defaults::RPC_MAX_CONCURRENT_STREAMS,
        }
    }
}

/// HTTP facade behaviour.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpConfig {
    /// Largest request body accepted, in mebibytes. Map images are the big ones.
    pub max_body_mb: usize,
    /// Request timeout, seconds; `0` disables it.
    pub request_timeout_s: f64,
    /// Origins allowed by CORS. Empty means no CORS headers are sent, which is
    /// what a same-origin deployment wants; the Vite development server needs
    /// its origin listed.
    pub cors_allow_origins: Vec<String>,
    /// Compress responses.
    pub compression: bool,
    /// SSE keep-alive interval, seconds.
    pub sse_keep_alive_s: f64,
    /// Maximum number of items a paged endpoint returns in one response.
    pub max_page_size: usize,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            max_body_mb: defaults::HTTP_MAX_BODY_MB,
            request_timeout_s: defaults::HTTP_REQUEST_TIMEOUT_S,
            cors_allow_origins: Vec::new(),
            compression: true,
            sse_keep_alive_s: defaults::HTTP_SSE_KEEP_ALIVE_S,
            max_page_size: defaults::HTTP_MAX_PAGE_SIZE,
        }
    }
}

impl HttpConfig {
    /// Rejects unusable limits.
    pub fn validate(&self) -> Result<()> {
        if self.max_body_mb == 0 {
            return Err(ServiceError::Config(
                "http.max_body_mb must be at least 1".to_string(),
            ));
        }
        if self.max_page_size == 0 {
            return Err(ServiceError::Config(
                "http.max_page_size must be at least 1".to_string(),
            ));
        }
        if self.request_timeout_s < 0.0 || self.sse_keep_alive_s < 0.0 {
            return Err(ServiceError::Config(
                "http.request_timeout_s and http.sse_keep_alive_s must not be negative".to_string(),
            ));
        }
        for origin in &self.cors_allow_origins {
            if !origin.starts_with("http://") && !origin.starts_with("https://") {
                return Err(ServiceError::Config(format!(
                    "http.cors_allow_origins entries must be full origins, got {origin:?}"
                )));
            }
        }
        Ok(())
    }

    /// Body limit in bytes.
    pub fn max_body_bytes(&self) -> usize {
        self.max_body_mb * 1024 * 1024
    }
}

/// Simulation execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SimulationConfigSection {
    /// Jobs allowed to run at once. `core` already parallelises inside a task, so
    /// this is deliberately small.
    pub max_concurrent: usize,
    /// Jobs allowed to wait; a submission beyond it is rejected with `busy`.
    pub queue_capacity: usize,
    /// Samples per frame when a result is streamed.
    pub stream_frame_samples: usize,
    /// Finished results kept in memory.
    pub keep_results: usize,
    /// Evaluate the metrics report of every run.
    pub with_metrics: bool,
}

impl Default for SimulationConfigSection {
    fn default() -> Self {
        Self {
            max_concurrent: defaults::SIM_MAX_CONCURRENT,
            queue_capacity: defaults::SIM_QUEUE_CAPACITY,
            stream_frame_samples: defaults::SIM_STREAM_FRAME_SAMPLES,
            keep_results: defaults::SIM_KEEP_RESULTS,
            with_metrics: true,
        }
    }
}

impl SimulationConfigSection {
    /// Rejects unusable limits.
    pub fn validate(&self) -> Result<()> {
        if self.max_concurrent == 0 {
            return Err(ServiceError::Config(
                "simulation.max_concurrent must be at least 1".to_string(),
            ));
        }
        if self.queue_capacity == 0 {
            return Err(ServiceError::Config(
                "simulation.queue_capacity must be at least 1".to_string(),
            ));
        }
        if self.stream_frame_samples == 0 {
            return Err(ServiceError::Config(
                "simulation.stream_frame_samples must be at least 1".to_string(),
            ));
        }
        Ok(())
    }
}

/// How the library is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageMode {
    /// Session only: everything is dropped when the process exits.
    Memory,
    /// Maps and results are kept under `data_dir`.
    Disk,
}

/// Where maps and results live.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Storage mode.
    pub mode: StorageMode,
    /// Root directory used in disk mode.
    pub data_dir: PathBuf,
    /// Maps kept in memory when the mode is `memory`.
    pub keep_maps: usize,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            mode: defaults::STORAGE_MODE,
            data_dir: PathBuf::from(defaults::STORAGE_DATA_DIR),
            keep_maps: defaults::STORAGE_KEEP_MAPS,
        }
    }
}

impl StorageConfig {
    /// Rejects an unusable directory.
    pub fn validate(&self) -> Result<()> {
        if self.keep_maps == 0 {
            return Err(ServiceError::Config(
                "storage.keep_maps must be at least 1".to_string(),
            ));
        }
        if self.mode == StorageMode::Disk && self.data_dir.as_os_str().is_empty() {
            return Err(ServiceError::Config(
                "storage.data_dir must not be empty in disk mode".to_string(),
            ));
        }
        Ok(())
    }
}

/// Static page behaviour.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebConfig {
    /// Answer unmatched non-API paths with `index.html`, so client-side routes
    /// survive a reload.
    pub spa_fallback: bool,
    /// `Cache-Control` header sent with the immutable, hashed build outputs.
    pub cache_control: String,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            spa_fallback: true,
            cache_control: defaults::WEB_CACHE_CONTROL.to_string(),
        }
    }
}

/// Validates a `host:port` string without resolving it.
fn validate_listen(field: &str, value: &str) -> Result<()> {
    let Some((host, port)) = value.rsplit_once(':') else {
        return Err(ServiceError::Config(format!(
            "{field} must be host:port, got {value:?}"
        )));
    };
    if host.is_empty() {
        return Err(ServiceError::Config(format!(
            "{field} has an empty host in {value:?}"
        )));
    }
    port.parse::<u16>()
        .map_err(|_| ServiceError::Config(format!("{field} has an unusable port in {value:?}")))?;
    Ok(())
}
