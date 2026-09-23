//! Service assembly: shared state, facade startup and shutdown.
//!
//! [`run`] is the whole lifecycle: build the state, start the facades the
//! configuration enables, wait for a shutdown signal, then give in-flight work a
//! bounded slice of time to finish. [`Service`] exposes the same steps separately
//! so a test can start a real service on ephemeral ports and stop it again.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use crate::api::dto::PageQuery;
use crate::api::{API_PREFIX, API_VERSION};
use crate::config::Config;
use crate::error::{Result, ServiceError};
use crate::store::{self, MapStore};
use crate::task::{TaskContext, TaskRegistry, TaskRunner};

/// Build facts reported by the system endpoints.
#[derive(Debug, Clone)]
pub struct BuildInfo {
    /// Version of this binary, from its manifest.
    pub service_version: &'static str,
    /// Version of the workspace, which every crate in it shares.
    pub workspace_version: &'static str,
    /// API path version.
    pub api_version: &'static str,
    /// Path prefix of the API.
    pub api_prefix: &'static str,
    /// Build time, RFC 3339.
    pub build_time: String,
    /// Whether the embedded page is a real front-end build.
    pub web_assets_built: bool,
    /// Number of embedded files.
    pub web_asset_count: usize,
    /// Total size of the embedded files, bytes.
    pub web_asset_bytes: usize,
}

impl BuildInfo {
    /// Facts of this build.
    pub fn current() -> Self {
        let epoch_s: i64 = env!("OUREALIS_BUILD_EPOCH_S").parse().unwrap_or_default();
        Self {
            service_version: env!("CARGO_PKG_VERSION"),
            workspace_version: env!("OUREALIS_WORKSPACE_VERSION"),
            api_version: API_VERSION,
            api_prefix: API_PREFIX,
            build_time: crate::api::time::unix_ms_to_rfc3339(epoch_s * 1000),
            web_assets_built: !crate::embed::is_placeholder(),
            web_asset_count: crate::embed::count(),
            web_asset_bytes: crate::embed::total_bytes(),
        }
    }
}

/// State every facade shares.
#[derive(Debug)]
pub struct AppState {
    /// Effective configuration.
    pub config: Config,
    /// Map library.
    pub maps: Arc<dyn MapStore>,
    /// Task registry: runs, plans and map builds share it.
    pub tasks: Arc<TaskRegistry>,
    /// Task admission and execution.
    pub runner: Arc<TaskRunner>,
    /// Build facts.
    pub build: BuildInfo,
    /// Start time of the process, Unix milliseconds.
    pub started_at_ms: i64,
}

impl AppState {
    /// Builds the state, opening the configured store.
    pub fn new(config: Config) -> Result<Arc<Self>> {
        let maps = store::open(&config.storage)?;
        let tasks = Arc::new(TaskRegistry::new(config.simulation.keep_results));
        let context = Arc::new(TaskContext {
            maps: Arc::clone(&maps),
            max_concurrent: config.simulation.max_concurrent,
            queue_capacity: config.simulation.queue_capacity,
            with_metrics: config.simulation.with_metrics,
        });
        let runner = Arc::new(TaskRunner::new(context, Arc::clone(&tasks)));
        Ok(Arc::new(Self {
            config,
            maps,
            tasks,
            runner,
            build: BuildInfo::current(),
            started_at_ms: crate::api::time::now_unix_ms(),
        }))
    }

    /// Clamps a page request to the configured maximum.
    ///
    /// A limit above the cap is clipped rather than rejected: a client asking for
    /// "everything" gets the largest page the service is willing to build, which
    /// is more useful than an error and cannot be used to force a huge response.
    pub fn page(&self, query: PageQuery) -> (usize, usize) {
        let limit = query.limit.clamp(1, self.config.http.max_page_size);
        (query.offset, limit)
    }

    /// Body size limit in bytes.
    pub fn max_body_bytes(&self) -> usize {
        self.config.http.max_body_bytes()
    }
}

/// A running service.
#[derive(Debug)]
pub struct Service {
    /// Shared state.
    state: Arc<AppState>,
    /// Address the HTTP facade bound, when it runs.
    http_addr: Option<SocketAddr>,
    /// Address the gRPC facade bound, when it runs.
    rpc_addr: Option<SocketAddr>,
    /// Tasks serving the facades.
    tasks: Vec<tokio::task::JoinHandle<Result<()>>>,
    /// Shutdown broadcaster.
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl Service {
    /// Starts the facades the configuration enables.
    ///
    /// The configuration is validated here as well as at load time: a caller that
    /// builds one in code never went through [`Config::from_toml`], and a
    /// configuration asking for the page without the API must fail before anything
    /// binds a port.
    pub async fn start(config: Config) -> Result<Self> {
        config.validate()?;
        let state = AppState::new(config)?;
        let (shutdown, _) = tokio::sync::watch::channel(false);
        let mut tasks = Vec::new();
        let mut http_addr = None;
        let mut rpc_addr = None;

        if state.config.server.http_enabled {
            let (handle, addr) =
                crate::facade::http::spawn(Arc::clone(&state), shutdown.subscribe()).await?;
            http_addr = Some(addr);
            tasks.push(handle);
        }
        if state.config.server.rpc_enabled {
            let (handle, addr) =
                crate::facade::rpc::spawn(Arc::clone(&state), shutdown.subscribe()).await?;
            rpc_addr = Some(addr);
            tasks.push(handle);
        }

        report_startup(&state, http_addr, rpc_addr);
        Ok(Self {
            state,
            http_addr,
            rpc_addr,
            tasks,
            shutdown,
        })
    }

    /// Shared state.
    pub fn state(&self) -> &Arc<AppState> {
        &self.state
    }

    /// Address the HTTP facade bound, when it runs.
    pub fn http_addr(&self) -> Option<SocketAddr> {
        self.http_addr
    }

    /// Address the gRPC facade bound, when it runs.
    pub fn rpc_addr(&self) -> Option<SocketAddr> {
        self.rpc_addr
    }

    /// Signals shutdown and waits for the facade tasks.
    ///
    /// A facade task that fails while shutting down is reported but does not turn
    /// the shutdown into a failure: the process is going away regardless.
    pub async fn shutdown(self, grace: Duration) -> Result<()> {
        let _ = self.shutdown.send(true);
        let deadline = tokio::time::Instant::now() + grace;
        for task in self.tasks {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, task).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(error))) => tracing::warn!("a facade stopped with an error: {error}"),
                Ok(Err(join)) => tracing::warn!("a facade task did not finish cleanly: {join}"),
                Err(_) => {
                    tracing::warn!(
                        "a facade did not stop within the {:.1} s grace period",
                        grace.as_secs_f64()
                    );
                }
            }
        }
        Ok(())
    }

    /// Waits for a shutdown signal (Ctrl-C or SIGTERM).
    pub async fn wait_for_signal(&self) {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let mut terminate = match signal(SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(error) => {
                    tracing::warn!("cannot listen for SIGTERM ({error}); Ctrl-C still works");
                    let _ = tokio::signal::ctrl_c().await;
                    return;
                }
            };
            tokio::select! {
                _ = tokio::signal::ctrl_c() => tracing::info!("interrupted"),
                _ = terminate.recv() => tracing::info!("terminated"),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("interrupted");
        }
    }
}

/// Runs the service until it is asked to stop.
pub async fn run(config: Config) -> Result<()> {
    let grace = Duration::from_secs_f64(config.server.shutdown_grace_s.max(0.0));
    let service = Service::start(config).await?;
    service.wait_for_signal().await;
    tracing::info!("shutting down");
    service.shutdown(grace).await
}

/// Logs what is being served, so an operator can see the effective switch state
/// without reading the configuration again.
fn report_startup(state: &AppState, http: Option<SocketAddr>, rpc: Option<SocketAddr>) {
    tracing::info!(
        "ourealis {} (api {}) starting",
        state.build.service_version,
        state.build.api_version
    );
    match http {
        Some(addr) => tracing::info!("http facade on http://{addr}{}", API_PREFIX),
        None => tracing::info!("http facade disabled"),
    }
    match rpc {
        Some(addr) => tracing::info!("rpc facade on grpc://{addr}"),
        None => tracing::info!("rpc facade disabled"),
    }
    if state.config.server.web_enabled {
        if state.build.web_assets_built {
            tracing::info!(
                "web page on http://{}/ ({} embedded file(s), {:.1} MiB)",
                http.map(|addr| addr.to_string())
                    .unwrap_or_else(|| "disabled".to_string()),
                state.build.web_asset_count,
                state.build.web_asset_bytes as f64 / (1024.0 * 1024.0)
            );
        } else {
            tracing::warn!(
                "the embedded page is the build placeholder; build the front-end in web/ and rebuild"
            );
        }
    } else {
        tracing::info!("web page disabled");
    }
    if !state.build.web_assets_built && state.config.server.web_enabled {
        // Already reported above; nothing else to say about the placeholder.
    }
    if state.build.workspace_version != state.build.service_version {
        tracing::warn!(
            "the binary is {} while the workspace is {}",
            state.build.service_version,
            state.build.workspace_version
        );
    }
}

/// Turns a missing map into a classified error with a hint about the library.
pub fn map_not_found(id: &str, available: usize) -> ServiceError {
    if available == 0 {
        ServiceError::not_found(format!("map {id} (the library is empty)"))
    } else {
        ServiceError::not_found(format!("map {id}"))
    }
}
