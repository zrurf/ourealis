//! Simulation jobs: submission, progress, results and exports.
//!
//! A submission is validated here before it reaches the queue — the individual,
//! the settings and the route shape — so a request that cannot run is refused
//! while the caller is still waiting. The run itself happens on the runner's
//! blocking pool; every result endpoint reads the finished
//! [`ourealis_core::SimulationOutput`] through an `Arc` and never mutates it.
//!
//! The sample streams live in [`crate::api::streams`] and the event transports in
//! [`crate::facade::sse`] and [`crate::facade::ws`]; their routes are merged here
//! so the job API is assembled in one place.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use ourealis_core::SimulationOutput;
use serde::{Deserialize, Serialize};

use crate::api::dto::result::{MetricSummary, SampleCountsDto, SummaryDto, TruthSampleDto};
use crate::api::dto::simulation::{
    JobStateDto, SimulationRequest, SimulationStateDto, SubmitReply,
};
use crate::api::dto::{Page, PageQuery};
use crate::api::error::{json_rejection, query_rejection};
use crate::api::maps::path_rejection;
use crate::api::streams::{attachment, truth_page};
use crate::app::AppState;
use crate::error::{Result, ServiceError};
use crate::job::Job;

/// Export formats of the result endpoint.
const EXPORT_FORMATS: [&str; 3] = ["json", "csv", "geojson"];

/// Query of the export endpoint.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExportQuery {
    /// Format: `json`, `csv` or `geojson`.
    #[serde(default)]
    pub format: Option<String>,
}

/// Two finished runs to compare.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompareRequest {
    /// Identifier of the first run.
    pub a: String,
    /// Identifier of the second run.
    pub b: String,
}

/// The headline numbers of one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComparedRun {
    /// Job identifier.
    pub id: String,
    /// Path ratio.
    pub path_ratio: f64,
    /// Mean moving speed, m/s.
    pub mean_speed_mps: f64,
    /// Step frequency, Hz.
    pub step_frequency_hz: f64,
    /// Coefficient of variation of the lap times, when the run had more than one
    /// lap.
    pub lap_time_cv: Option<f64>,
    /// 95th percentile of the turn rate, rad/s.
    pub turn_rate_p95: f64,
    /// Mean absolute effective curvature, per metre.
    pub mean_kappa_eff: f64,
    /// Route length, metres.
    pub route_length_m: f64,
    /// Duration, seconds.
    pub duration_s: f64,
    /// Moving-speed samples the comparison used.
    pub speed_samples: usize,
}

/// Difference between two runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareReply {
    /// The first run, the baseline.
    pub a: ComparedRun,
    /// The second run.
    pub b: ComparedRun,
    /// `b - a` for every metric the two runs share.
    pub delta: CompareDelta,
    /// Two-sample Kolmogorov–Smirnov test of the moving-speed distributions.
    pub speed_ks: KsResult,
}

/// Metric differences, `b - a`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareDelta {
    /// Difference in path ratio.
    pub path_ratio: f64,
    /// Difference in mean speed, m/s.
    pub mean_speed_mps: f64,
    /// Difference in step frequency, Hz.
    pub step_frequency_hz: f64,
    /// Difference in lap-time coefficient of variation, when both runs have one.
    pub lap_time_cv: Option<f64>,
    /// Difference in the 95th percentile of the turn rate, rad/s.
    pub turn_rate_p95: f64,
    /// Difference in mean absolute effective curvature, per metre.
    pub mean_kappa_eff: f64,
    /// Difference in route length, metres.
    pub route_length_m: f64,
    /// Difference in duration, seconds.
    pub duration_s: f64,
}

/// Outcome of the two-sample test.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct KsResult {
    /// KS statistic: the largest gap between the two empirical distributions.
    pub statistic: f64,
    /// Asymptotic p-value of the statistic at these sample counts.
    pub p_value: f64,
    /// Samples the first run contributed.
    pub samples_a: usize,
    /// Samples the second run contributed.
    pub samples_b: usize,
}

/// Lists jobs, newest first.
pub async fn list(
    State(state): State<Arc<AppState>>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<Page<SimulationStateDto>>> {
    let query = query.map_err(query_rejection)?.0;
    let (offset, limit) = state.page(query);
    let all = state.jobs.list();
    let total = all.len();
    let items = all
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|job| job.to_dto())
        .collect();
    Ok(Json(Page::new(items, total, offset)))
}

/// Submits a run.
pub async fn submit(
    State(state): State<Arc<AppState>>,
    body: Result<Json<SimulationRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<SubmitReply>)> {
    let request = body.map_err(json_rejection)?.0;
    validate_request(&request)?;
    let id = crate::store::identifier(request.name.as_deref().unwrap_or("run"));
    let job = state.runner.submit(id, request).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(SubmitReply {
            id: job.id().to_string(),
            state: job.state(),
        }),
    ))
}

/// Checks everything about a request that does not need the map.
///
/// Shared by the two facades: a request is refused with the same message whether
/// it arrives over HTTP or over gRPC.
pub(crate) fn validate_request(request: &SimulationRequest) -> Result<()> {
    request.person.resolve()?;
    let mut settings = ourealis_core::sim::SimulationConfig::default();
    request.settings.apply(&mut settings)?;
    settings.sensors.validate()?;
    match &request.route {
        crate::api::dto::simulation::RouteSpec::Standard { .. } => {
            request.standard_request()?;
        }
        crate::api::dto::simulation::RouteSpec::Loop { .. } => {
            request.loop_request()?;
        }
        crate::api::dto::simulation::RouteSpec::Dynamic { .. } => {
            request.dynamic_request()?;
        }
    }
    Ok(())
}

/// State of one job.
pub async fn state_of(
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
) -> Result<Json<SimulationStateDto>> {
    let id = path.map_err(path_rejection)?.0;
    Ok(Json(state.jobs.get(&id)?.to_dto()))
}

/// Cancels a job.
pub async fn cancel(
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
) -> Result<StatusCode> {
    let id = path.map_err(path_rejection)?.0;
    let job = state.jobs.get(&id)?;
    if !job.cancel() {
        return Err(ServiceError::Conflict(format!(
            "simulation {id} already finished with state {}",
            job.state().to_string_name()
        )));
    }
    tracing::info!("simulation {id} cancellation requested");
    Ok(StatusCode::NO_CONTENT)
}

/// Manifest, metrics and counts of a finished run.
pub async fn summary(
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
) -> Result<Json<SummaryDto>> {
    let id = path.map_err(path_rejection)?.0;
    let job = state.jobs.get(&id)?;
    let output = finished(&job)?;
    Ok(Json(build_summary(&id, &output)?))
}

/// A page of ground-truth samples.
pub async fn truth(
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<Page<TruthSampleDto>>> {
    let id = path.map_err(path_rejection)?.0;
    let query = query.map_err(query_rejection)?.0;
    let (offset, limit) = state.page(query);
    let job = state.jobs.get(&id)?;
    let output = finished(&job)?;
    Ok(Json(truth_page(&output, offset, limit)))
}

/// Exports a finished run.
pub async fn export(
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
    query: Result<Query<ExportQuery>, QueryRejection>,
) -> Result<Response> {
    let id = path.map_err(path_rejection)?.0;
    let format = query
        .map_err(query_rejection)?
        .0
        .format
        .unwrap_or_else(|| "json".to_string());
    let (content_type, extension) = match format.as_str() {
        "json" => ("application/json", "json"),
        "csv" => ("text/csv; charset=utf-8", "csv"),
        "geojson" => ("application/geo+json", "geojson"),
        other => {
            return Err(ServiceError::Invalid(format!(
                "unknown export format {other:?}; expected one of {}",
                EXPORT_FORMATS.join(", ")
            )));
        }
    };
    let job = state.jobs.get(&id)?;
    let output = finished(&job)?;
    // A geo-referenced map turns the track into degrees; a purely local map keeps
    // metres and says so in the document's own properties.
    let frame = state
        .maps
        .get(job.map_id())
        .ok()
        .and_then(|entry| entry.open().ok())
        .filter(|map| map.has_geo_reference())
        .map(|map| ourealis_core::math::LocalFrame::from_header(map.header()));
    let document = export_document(&output, &format, frame.as_ref())?;
    Ok(attachment(
        document,
        content_type,
        &format!("{id}.{extension}"),
    ))
}

/// Compares the metrics of two finished runs.
pub async fn compare(
    State(state): State<Arc<AppState>>,
    body: Result<Json<CompareRequest>, JsonRejection>,
) -> Result<Json<CompareReply>> {
    let request = body.map_err(json_rejection)?.0;
    let first = state.jobs.get(&request.a)?;
    let second = state.jobs.get(&request.b)?;
    let first = finished(&first)?;
    let second = finished(&second)?;
    let samples_a = ourealis_core::eval::speed_samples(&first.trajectory);
    let samples_b = ourealis_core::eval::speed_samples(&second.trajectory);
    let statistic = ourealis_core::eval::ks_statistic(&samples_a, &samples_b);
    let p_value = ourealis_core::eval::ks_p_value(statistic, samples_a.len(), samples_b.len());
    let a = compared_run(&request.a, &first, samples_a.len());
    let b = compared_run(&request.b, &second, samples_b.len());
    Ok(Json(CompareReply {
        delta: CompareDelta {
            path_ratio: b.path_ratio - a.path_ratio,
            mean_speed_mps: b.mean_speed_mps - a.mean_speed_mps,
            step_frequency_hz: b.step_frequency_hz - a.step_frequency_hz,
            lap_time_cv: match (a.lap_time_cv, b.lap_time_cv) {
                (Some(first), Some(second)) => Some(second - first),
                _ => None,
            },
            turn_rate_p95: b.turn_rate_p95 - a.turn_rate_p95,
            mean_kappa_eff: b.mean_kappa_eff - a.mean_kappa_eff,
            route_length_m: b.route_length_m - a.route_length_m,
            duration_s: b.duration_s - a.duration_s,
        },
        speed_ks: KsResult {
            statistic,
            p_value,
            samples_a: samples_a.len(),
            samples_b: samples_b.len(),
        },
        a,
        b,
    }))
}

/// Every route of the job API, including the streaming routes and the two whose
/// bodies the facades own (SSE and WebSocket).
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/simulations", get(list).post(submit))
        .route("/simulations/compare", post(compare))
        .route("/simulations/{id}", get(state_of).delete(cancel))
        .route("/simulations/{id}/summary", get(summary))
        .route("/simulations/{id}/truth", get(truth))
        .route("/simulations/{id}/export", get(export))
        .merge(crate::api::streams::router())
        .merge(crate::facade::sse::routes())
        .merge(crate::facade::ws::routes())
}

/// The result of a finished job, or the reason it cannot be read.
///
/// A result evicted from memory is `unsupported` rather than `not_found`: the job
/// exists and its seed reproduces the run exactly, so resubmitting is a real
/// remedy and a 404 would be misleading.
pub(crate) fn finished(job: &Job) -> Result<Arc<SimulationOutput>> {
    match job.state() {
        JobStateDto::Succeeded => job.result().ok_or_else(|| {
            ServiceError::Unsupported(format!(
                "the result of simulation {} was evicted from memory; resubmit it to reproduce the run from its seed",
                job.id()
            ))
        }),
        JobStateDto::Failed => Err(ServiceError::Conflict(format!(
            "simulation {} failed: {}",
            job.id(),
            job.to_dto().error.unwrap_or_else(|| "no message".to_string())
        ))),
        JobStateDto::Cancelled => Err(ServiceError::Conflict(format!(
            "simulation {} was cancelled and has no result",
            job.id()
        ))),
        state => Err(ServiceError::Conflict(format!(
            "simulation {} is {} and has no result yet",
            job.id(),
            state.to_string_name()
        ))),
    }
}

/// Builds the summary of a finished run.
pub(crate) fn build_summary(id: &str, output: &SimulationOutput) -> Result<SummaryDto> {
    let metrics = output.metrics.as_ref();
    Ok(SummaryDto {
        id: id.to_string(),
        route_length_m: output.route.length_m,
        duration_s: output.duration_s(),
        samples: SampleCountsDto {
            truth: output.truth.len(),
            gnss: output.sensors.gnss.len(),
            accel: output.sensors.imu.accel.len(),
            gyro: output.sensors.imu.gyro.len(),
            mag: output.sensors.mag.len(),
            baro: output.sensors.baro.len(),
        },
        backend: output.backend_name().to_string(),
        metrics: metrics.map(|report| MetricSummary {
            path_ratio: Some(report.path_ratio),
            mean_speed_mps: Some(report.speed.mean),
            // The report carries the measured spectrum rather than the nominal
            // frequency, so the step frequency comes from the trajectory that
            // produced both.
            step_frequency_hz: Some(output.trajectory.step_frequency),
            lap_time_cv: report.lap_time_cv,
            turn_rate_p95: Some(report.turn_rate.p95),
            mean_kappa_eff: Some(report.mean_abs_curvature),
        }),
        manifest: serde_json::to_value(&output.manifest)?,
        report: metrics.map(serde_json::to_value).transpose()?,
    })
}

/// Renders a run through the simulator's own exporters.
///
/// Every writer of `sim::export` writes a file, so each format goes through a
/// scratch directory under the system temporary directory and is read back. The
/// CSV writer produces one file per stream; those are concatenated into one
/// document, each section introduced by a `# <stream>` line.
fn export_document(
    output: &SimulationOutput,
    format: &str,
    frame: Option<&ourealis_core::math::LocalFrame>,
) -> Result<Vec<u8>> {
    let directory = scratch_dir()?;
    let result = (|| -> Result<Vec<u8>> {
        match format {
            "json" => {
                let path = directory.join("run.json");
                ourealis_core::sim::export::write_json(output, &path)?;
                Ok(std::fs::read(&path)?)
            }
            "geojson" => {
                let path = directory.join("run.geojson");
                ourealis_core::sim::export::write_geojson(output, frame, &path)?;
                Ok(std::fs::read(&path)?)
            }
            "csv" => {
                ourealis_core::sim::export::write_csv_dir(output, &directory)?;
                let mut document = Vec::new();
                for name in ["truth", "gnss", "accel", "gyro", "mag", "baro"] {
                    let path = directory.join(format!("{name}.csv"));
                    let text = std::fs::read(&path)?;
                    writeln!(document, "# {name}").map_err(std::io::Error::other)?;
                    document.extend_from_slice(&text);
                }
                Ok(document)
            }
            other => Err(ServiceError::Invalid(format!(
                "unknown export format {other:?}; expected one of {}",
                EXPORT_FORMATS.join(", ")
            ))),
        }
    })();
    if let Err(error) = std::fs::remove_dir_all(&directory) {
        tracing::warn!(
            "the export scratch directory {} could not be removed: {error}",
            directory.display()
        );
    }
    result
}

/// Creates a unique scratch directory for one export.
fn scratch_dir() -> Result<PathBuf> {
    let path =
        std::env::temp_dir().join(format!("ourealis-export-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

/// The headline numbers of one run.
fn compared_run(id: &str, output: &SimulationOutput, speed_samples: usize) -> ComparedRun {
    let metrics = output.metrics.as_ref();
    ComparedRun {
        id: id.to_string(),
        path_ratio: metrics.map(|report| report.path_ratio).unwrap_or(0.0),
        mean_speed_mps: metrics.map(|report| report.speed.mean).unwrap_or(0.0),
        step_frequency_hz: output.trajectory.step_frequency,
        lap_time_cv: metrics.and_then(|report| report.lap_time_cv),
        turn_rate_p95: metrics.map(|report| report.turn_rate.p95).unwrap_or(0.0),
        mean_kappa_eff: metrics
            .map(|report| report.mean_abs_curvature)
            .unwrap_or(0.0),
        route_length_m: output.route.length_m,
        duration_s: output.duration_s(),
        speed_samples,
    }
}
