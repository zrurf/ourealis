//! Liveness and capability endpoints.
//!
//! `/health` answers the probe, `/system/info` describes the build and the
//! capability bits a client needs before it submits anything, and
//! `/system/config` returns the effective configuration so an operator can see
//! what a running process actually reads. There are no credentials in the
//! configuration, so nothing has to be redacted.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::get;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::error::Result;

/// Person presets this build accepts, in the order the form should offer them.
pub const PRESETS: [&str; 3] = ["jog", "moderate", "race"];

/// Reply of the liveness probe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthDto {
    /// `ok` while the process serves requests.
    pub status: String,
}

/// Build and capability information of the running service.
///
/// The field names match `ourealis.api.v1.SystemInfo`; the gRPC facade maps this
/// type onto that message rather than defining a second shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemInfoDto {
    /// Version of this binary, from its manifest.
    pub version: String,
    /// Version of the workspace, shared by every crate in it.
    pub workspace_version: String,
    /// Version of the linked simulator crate.
    pub core_version: String,
    /// Version of the linked map-format crate.
    pub map_format_version: String,
    /// API path version.
    pub api_version: String,
    /// Path prefix every API route lives under.
    pub api_prefix: String,
    /// Build time, RFC 3339.
    pub build_time: String,
    /// Whether the gRPC facade is enabled.
    pub rpc_enabled: bool,
    /// Whether the HTTP facade is enabled.
    pub http_enabled: bool,
    /// Whether the static page is enabled.
    pub web_enabled: bool,
    /// Compute backend policy applied to a run that does not name one.
    ///
    /// A run resolves its device at start and records it in its own manifest, so
    /// this is the configured policy rather than the device of any particular
    /// run.
    pub backend: String,
    /// Threads the process may run simulation stages on.
    pub worker_threads: u32,
    /// Person parameter presets this build accepts.
    pub presets: Vec<String>,
    /// Whether the embedded page is a real front-end build.
    pub web_assets_built: bool,
    /// Number of embedded page files.
    pub web_asset_files: usize,
    /// Total size of the embedded page files, bytes.
    pub web_asset_bytes: usize,
    /// Storage mode in effect: `memory` or `disk`.
    pub storage_mode: String,
    /// Maps in the library at the time of the reply.
    pub maps: usize,
    /// Jobs queued or running at the time of the reply.
    pub simulations_in_flight: usize,
}

/// Liveness probe.
pub async fn health() -> Json<HealthDto> {
    Json(HealthDto {
        status: "ok".to_string(),
    })
}

/// Build facts and capability bits.
pub async fn info(State(state): State<Arc<AppState>>) -> Json<SystemInfoDto> {
    Json(info_dto(&state))
}

/// Build facts and capability bits, as both facades report them.
pub(crate) fn info_dto(state: &AppState) -> SystemInfoDto {
    let build = &state.build;
    SystemInfoDto {
        version: build.service_version.to_string(),
        workspace_version: build.workspace_version.to_string(),
        // Every crate in the workspace carries the `[workspace.package]` version,
        // so the one the build script read is the version of both links.
        core_version: build.workspace_version.to_string(),
        map_format_version: build.workspace_version.to_string(),
        api_version: build.api_version.to_string(),
        api_prefix: build.api_prefix.to_string(),
        build_time: build.build_time.clone(),
        rpc_enabled: state.config.server.rpc_enabled,
        http_enabled: state.config.server.http_enabled,
        web_enabled: state.config.server.web_enabled,
        backend: backend_policy_name(),
        worker_threads: worker_threads(),
        presets: PRESETS.iter().map(|name| name.to_string()).collect(),
        web_assets_built: build.web_assets_built,
        web_asset_files: build.web_asset_count,
        web_asset_bytes: build.web_asset_bytes,
        storage_mode: match state.config.storage.mode {
            crate::config::StorageMode::Memory => "memory",
            crate::config::StorageMode::Disk => "disk",
        }
        .to_string(),
        maps: state.maps.list().len(),
        simulations_in_flight: state.runner.in_flight(),
    }
}

/// The effective configuration, as TOML's data model in JSON.
pub async fn config(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>> {
    Ok(Json(serde_json::to_value(&state.config)?))
}

/// Every route of this module.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/system/info", get(info))
        .route("/system/config", get(config))
}

/// Name of the compute backend policy a run gets when it does not name one.
///
/// A request may still name another policy; this is the default the service
/// applies, not the device any particular run resolved.
fn backend_policy_name() -> String {
    ourealis_core::sim::output::backend_name(ourealis_core::sim::Backend::Auto).to_string()
}

/// Threads the process may use for simulation stages.
///
/// The simulator parallelises through its own pool, whose width is the machine's
/// parallelism unless the environment overrides it; the logical processor count
/// is what the service can state without probing a device.
fn worker_threads() -> u32 {
    std::thread::available_parallelism()
        .map(|count| count.get() as u32)
        .unwrap_or(1)
}
