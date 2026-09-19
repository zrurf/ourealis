//! Simulation output and its manifest.
//!
//! The manifest is part of the deliverable rather than a debugging aid: it
//! records the seed, the individual parameters, the map identity and the compute
//! backend, which is what makes a run reproducible and a comparison between two
//! runs meaningful.

use serde::{Deserialize, Serialize};

use crate::eval::MetricsReport;
use crate::motion::Trajectory;
use crate::person::PersonParams;
use crate::sensor::{Sensors, TruthState};

use super::config::Backend;

/// One candidate route considered during planning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateSummary {
    /// Cost in equivalent metres.
    pub cost_equiv_m: f64,
    /// Length in metres.
    pub length_m: f64,
    /// Path-size factor.
    pub path_size: f64,
    /// Choice probability under the Logit model.
    pub probability: f64,
}

/// The route that was planned and chosen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteSummary {
    /// Sampled path points, metres.
    pub points: Vec<[f64; 2]>,
    /// Total length, metres.
    pub length_m: f64,
    /// Chosen-route cost in equivalent metres.
    pub cost_equiv_m: f64,
    /// Candidate sets, one per leg.
    pub legs: Vec<Vec<CandidateSummary>>,
    /// Index of the chosen candidate in each leg.
    pub chosen: Vec<usize>,
}

impl RouteSummary {
    /// Builds the summary of a planned route.
    pub fn from_route(route: &crate::plan::PlannedRoute) -> Self {
        let legs = route
            .legs
            .iter()
            .map(|leg| {
                let probabilities = leg.candidates.probabilities();
                leg.candidates
                    .candidates
                    .iter()
                    .enumerate()
                    .map(|(index, candidate)| CandidateSummary {
                        cost_equiv_m: candidate.cost_equiv_m,
                        length_m: candidate.length_m,
                        path_size: candidate.path_size,
                        probability: probabilities.get(index).copied().unwrap_or(0.0),
                    })
                    .collect()
            })
            .collect();
        Self {
            points: route
                .path
                .points()
                .iter()
                .map(|point| [point.x, point.y])
                .collect(),
            length_m: route.length_m,
            cost_equiv_m: route.cost_equiv_m,
            legs,
            chosen: route.legs.iter().map(|leg| leg.chosen).collect(),
        }
    }
}

/// Provenance and parameters of one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunManifest {
    /// Crate version that produced the run.
    pub generator: String,
    /// Global seed.
    pub seed: u64,
    /// Individual index inside the batch.
    pub individual: u32,
    /// Planning mode name.
    pub mode: String,
    /// Map name, when the map carries one.
    pub map_name: Option<String>,
    /// Individual parameters.
    pub person: PersonParams,
    /// Requested start point, metres.
    pub start: [f64; 2],
    /// Requested goal, metres.
    pub goal: [f64; 2],
    /// Sensor rates actually used.
    pub rates_hz: [f64; 4],
    /// Compute backend that produced the run.
    pub backend: String,
    /// Inertial mount.
    pub mount: String,
    /// Recording duration, seconds.
    pub duration_s: f64,
}

/// Everything one run produces.
#[derive(Debug, Clone)]
pub struct SimulationOutput {
    /// Provenance and parameters.
    pub manifest: RunManifest,
    /// Ground truth at the inertial rate.
    pub truth: Vec<TruthState>,
    /// Simulated sensor streams.
    pub sensors: Sensors,
    /// Planned route.
    pub route: RouteSummary,
    /// Evaluation report, when enabled.
    pub metrics: Option<MetricsReport>,
    /// The motion trajectory, kept for callers that need sub-sample access.
    pub trajectory: Trajectory,
}

impl SimulationOutput {
    /// Number of ground-truth samples.
    pub fn truth_len(&self) -> usize {
        self.truth.len()
    }

    /// Duration of the recording, seconds.
    pub fn duration_s(&self) -> f64 {
        self.trajectory.duration_s()
    }

    /// Backend name recorded in the manifest.
    pub fn backend_name(&self) -> &str {
        &self.manifest.backend
    }

    /// Serialises the manifest and metrics as JSON.
    pub fn summary_json(&self) -> String {
        #[derive(Serialize)]
        struct Summary<'a> {
            manifest: &'a RunManifest,
            metrics: Option<&'a MetricsReport>,
            route_length_m: f64,
            sample_counts: SampleCounts,
        }
        #[derive(Serialize)]
        struct SampleCounts {
            truth: usize,
            gnss: usize,
            accel: usize,
            gyro: usize,
            mag: usize,
            baro: usize,
        }
        let summary = Summary {
            manifest: &self.manifest,
            metrics: self.metrics.as_ref(),
            route_length_m: self.route.length_m,
            sample_counts: SampleCounts {
                truth: self.truth.len(),
                gnss: self.sensors.gnss.len(),
                accel: self.sensors.imu.accel.len(),
                gyro: self.sensors.imu.gyro.len(),
                mag: self.sensors.mag.len(),
                baro: self.sensors.baro.len(),
            },
        };
        serde_json::to_string_pretty(&summary).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Name of a backend policy, used when no device-backed backend was resolved.
///
/// The CPU policy always selects the rayon implementation, so it is named after it
/// rather than after the policy: a manifest that said "cpu" for one run and
/// "cpu-rayon" for another would describe the same backend two ways.
pub fn backend_name(backend: Backend) -> &'static str {
    match backend {
        Backend::Auto => "auto",
        Backend::Cpu => "cpu-rayon",
        Backend::Gpu => "gpu",
    }
}
