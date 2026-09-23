//! Result resources: samples, summaries, events and route previews.

use serde::{Deserialize, Serialize};

use super::Vec2;

/// One ground-truth sample.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TruthSampleDto {
    /// Time since the start of the recording, seconds.
    pub time_s: f64,
    /// Reported position including jitter and bounce, metres.
    pub position: Vec2,
    /// Low-frequency centre-of-mass position, metres.
    pub position_low: Vec2,
    /// Altitude including bounce, metres.
    pub z: f64,
    /// Terrain elevation, metres.
    pub terrain_z: f64,
    /// Speed along the path, m/s.
    pub speed: f64,
    /// Body heading, radians.
    pub heading_rad: f64,
    /// Head heading, radians.
    pub head_heading_rad: f64,
    /// Pitch, radians.
    pub pitch_rad: f64,
    /// Roll, radians.
    pub roll_rad: f64,
    /// Effective curvature, per metre.
    pub kappa_eff: f64,
    /// Lateral offset from the centre line, metres.
    pub offset_m: f64,
    /// Terrain grade along the direction of travel, rise over run.
    pub grade: f64,
    /// True while standing.
    pub standing: bool,
    /// True while turning on the spot.
    pub turning: bool,
    /// Low-frequency velocity in the world frame, m/s.
    pub velocity: [f64; 3],
    /// Low-frequency acceleration in the world frame, m/s^2.
    pub acceleration: [f64; 3],
}

/// One sample of a sensor stream.
///
/// One shape for all five streams: the vector channels use [`SensorSampleDto::v`]
/// and the scalar ones use their named fields, so a client can plot any channel
/// without a per-channel type.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SensorSampleDto {
    /// Time since the start of the recording, seconds.
    pub time_s: f64,
    /// Channel name: `gnss`, `accel`, `gyro`, `mag` or `baro`.
    pub channel: &'static str,
    /// Vector payload: position for GNSS, body-frame acceleration, angular rate
    /// or magnetic field, empty for the barometer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub v: Option<[f64; 3]>,
    /// Latitude, degrees, GNSS only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latitude_deg: Option<f64>,
    /// Longitude, degrees, GNSS only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub longitude_deg: Option<f64>,
    /// Altitude, metres, GNSS only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub altitude_m: Option<f64>,
    /// Ground speed, m/s, GNSS only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_mps: Option<f64>,
    /// Course over ground, radians, GNSS only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading_rad: Option<f64>,
    /// Whether the fix survived the dropout model, GNSS only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid: Option<bool>,
    /// Satellite count, GNSS only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub satellites: Option<u8>,
    /// Pressure, pascals, barometer only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressure_pa: Option<f64>,
}

/// Sample counts of one run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct SampleCountsDto {
    /// Ground-truth samples.
    pub truth: usize,
    /// GNSS fixes.
    pub gnss: usize,
    /// Accelerometer samples.
    pub accel: usize,
    /// Gyroscope samples.
    pub gyro: usize,
    /// Magnetometer samples.
    pub mag: usize,
    /// Barometer samples.
    pub baro: usize,
}

/// The headline numbers of a run's evaluation report.
///
/// The full report has dozens of fields and is returned as JSON by the summary
/// endpoint; this is the subset a list or a badge needs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MetricSummary {
    /// Path ratio: route length over straight-line distance.
    pub path_ratio: Option<f64>,
    /// Mean speed over the running sections, m/s.
    pub mean_speed_mps: Option<f64>,
    /// Step frequency, Hz.
    pub step_frequency_hz: Option<f64>,
    /// Coefficient of variation of the lap times, dimensionless.
    pub lap_time_cv: Option<f64>,
    /// 95th percentile of the turn rate, rad/s.
    pub turn_rate_p95: Option<f64>,
    /// Mean absolute effective curvature, per metre.
    pub mean_kappa_eff: Option<f64>,
}

/// Summary of a finished run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryDto {
    /// Task identifier.
    pub id: String,
    /// Route length, metres.
    pub route_length_m: f64,
    /// Duration of the recording, seconds.
    pub duration_s: f64,
    /// Sample counts per stream.
    pub samples: SampleCountsDto,
    /// Backend the run resolved to.
    pub backend: String,
    /// Headline metrics, absent when the run did not evaluate any.
    pub metrics: Option<MetricSummary>,
    /// Full manifest as recorded by the simulator.
    pub manifest: serde_json::Value,
    /// Full evaluation report, absent when the run did not evaluate one.
    pub report: Option<serde_json::Value>,
}

/// One route candidate of a preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutePreviewCandidate {
    /// Route length, metres.
    pub length_m: f64,
    /// Route cost in equivalent metres.
    pub cost_equiv_m: f64,
    /// Probability the Logit choice model gives this candidate.
    pub probability: f64,
    /// Path size factor, which penalises overlap between candidates.
    pub path_size: f64,
    /// Whether this candidate came from the map's stored library.
    pub from_library: bool,
    /// Polyline of the candidate.
    pub points: Vec<Vec2>,
}

/// A route preview: planning only, before any motion or sensor work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutePreview {
    /// Candidate routes, best first.
    pub candidates: Vec<RoutePreviewCandidate>,
    /// Index of the candidate the Logit draw selected.
    pub chosen: usize,
    /// Total length of the chosen route, metres.
    pub length_m: f64,
    /// Cost of the chosen route in equivalent metres.
    pub cost_equiv_m: f64,
    /// Straight-line distance from start to goal, metres.
    pub straight_line_m: f64,
    /// Time the planning step took, milliseconds.
    pub planning_ms: f64,
    /// The smoothed path actually used for motion, when smoothing ran.
    pub path: Vec<Vec2>,
    /// Speed limit along the smoothed path, m/s, sampled at the profile grid.
    pub speed_limit_mps: Vec<f64>,
    /// Arc length of each speed limit sample, metres.
    pub speed_limit_s: Vec<f64>,
}

/// A progress or log event of a task.
///
/// The same shape travels over SSE, WebSocket and the gRPC `Watch` stream, so a
/// client can switch transports without changing its parser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventDto {
    /// The task changed state.
    State {
        /// State after the change.
        state: String,
        /// Stage the task is in.
        stage: String,
        /// Progress, 0 to 1, or `None` while the service cannot say.
        progress: Option<f64>,
        /// Wall-clock time the task has been running, seconds.
        elapsed_s: f64,
    },
    /// The task entered a new pipeline stage.
    Stage {
        /// Stage name.
        stage: String,
        /// Progress, 0 to 1, or `None` while the service cannot say.
        progress: Option<f64>,
        /// Wall-clock time the task has been running, seconds.
        elapsed_s: f64,
    },
    /// A log line produced by the task.
    Log {
        /// Level: `trace`, `debug`, `info`, `warn` or `error`.
        level: String,
        /// Message, English.
        message: String,
        /// Wall-clock time the task has been running, seconds.
        elapsed_s: f64,
    },
    /// The task finished successfully.
    Done {
        /// Final state.
        state: String,
        /// URL of the summary resource.
        summary_url: String,
    },
    /// The task failed.
    Error {
        /// Failure classification.
        kind: String,
        /// Failure message, English.
        message: String,
    },
}

impl EventDto {
    /// Name of the event, matching the SSE `event:` field.
    pub fn name(&self) -> &'static str {
        match self {
            EventDto::State { .. } => "state",
            EventDto::Stage { .. } => "stage",
            EventDto::Log { .. } => "log",
            EventDto::Done { .. } => "done",
            EventDto::Error { .. } => "error",
        }
    }
}
