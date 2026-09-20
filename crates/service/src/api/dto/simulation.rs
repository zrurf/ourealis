//! Simulation resources: requests, settings, individuals and job state.

use ourealis_core::motion::MotionConfig;
use ourealis_core::motion::limits::LookAheadMode;
use ourealis_core::person::{PaceStrategy, PersonParams, Preset};
use ourealis_core::plan::{LoopRequest, StandardRequest, Waypoint};
use ourealis_core::sensor::DeviceMount;
use ourealis_core::sim::{Backend, SimulationConfig};
use ourealis_map_format::MotionMode;
use serde::{Deserialize, Serialize};

use crate::api::dto::Vec2;
use crate::error::{Result, ServiceError};

use ourealis_core::plan::ViaSemantics;

/// Lifecycle state of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStateDto {
    /// Waiting for a worker slot.
    Queued,
    /// Running; `stage` says how far it has come.
    Running,
    /// Finished successfully.
    Succeeded,
    /// Finished with an error.
    Failed,
    /// Cancelled before finishing.
    Cancelled,
}

impl JobStateDto {
    /// True once the job cannot change state again.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobStateDto::Succeeded | JobStateDto::Failed | JobStateDto::Cancelled
        )
    }
}

/// State of one job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimulationStateDto {
    /// Job identifier.
    pub id: String,
    /// Lifecycle state.
    pub state: JobStateDto,
    /// Pipeline stage: `queued`, `environment`, `planning`, `motion`, `sensors`,
    /// `metrics` or `done`.
    pub stage: String,
    /// Fraction of the run completed, 0 to 1, or `None` while the service cannot
    /// say. The simulator's run is one call, so a running job reports `None` and
    /// the client shows elapsed time instead of a made-up fraction.
    pub progress: Option<f64>,
    /// Wall-clock time the job has been running, seconds, absent while queued.
    pub elapsed_s: Option<f64>,
    /// Name given at submission, when one was given.
    pub name: Option<String>,
    /// Map the run used.
    pub map_id: String,
    /// Planning mode: `standard`, `loop` or `dynamic`.
    pub mode: String,
    /// Failure message, absent unless the state is `failed`.
    pub error: Option<String>,
    /// Failure classification, absent unless the state is `failed`.
    pub error_kind: Option<String>,
    /// Submission time, RFC 3339.
    pub created_at: String,
    /// Time the worker started, RFC 3339, absent while queued.
    pub started_at: Option<String>,
    /// Time the job finished, RFC 3339, absent while it runs.
    pub finished_at: Option<String>,
}

/// Reply to a submission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubmitReply {
    /// Identifier of the queued job.
    pub id: String,
    /// State right after submission, normally `queued`.
    pub state: JobStateDto,
}

/// Where the map of a run comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MapRef {
    /// A map already in the library.
    Id {
        /// Library identifier.
        id: String,
    },
    /// A synthetic map built for this run.
    Synthetic {
        /// Generator settings.
        spec: SyntheticSpec,
    },
    /// An OMF image carried in the request, base64 encoded.
    Inline {
        /// Image bytes, base64.
        omf_base64: String,
        /// Name to report the map under.
        #[serde(default)]
        name: Option<String>,
    },
}

/// Settings of the synthetic map generator.
///
/// The defaults match `SyntheticMapSpec::default()`; `preset` selects one of the
/// shapes the simulator's own tests use.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SyntheticSpec {
    /// Shape preset: `default`, `compact` or `wide`.
    pub preset: String,
    /// Map width, metres. Ignored when a preset is named.
    pub width_m: f64,
    /// Map height, metres.
    pub height_m: f64,
    /// Cell resolution, metres.
    pub resolution_m: f64,
    /// Chunk side length in cells.
    pub chunk_size: u16,
    /// Seed of the generator.
    pub seed: u64,
    /// Include a candidate path library.
    pub with_kpath_library: bool,
}

impl Default for SyntheticSpec {
    fn default() -> Self {
        Self {
            preset: "default".to_string(),
            width_m: 600.0,
            height_m: 400.0,
            resolution_m: 1.0,
            chunk_size: 256,
            seed: 0x0DDB_1A5E,
            with_kpath_library: true,
        }
    }
}

impl SyntheticSpec {
    /// Builds the simulator's own specification.
    ///
    /// `preset` picks the base shape and fixes its footprint: `compact` is the
    /// 300 x 200 m fixture the simulator's own tests use, `default` (or an empty
    /// string) is the 600 x 400 m campus, and **any other value** builds a custom
    /// map from `width_m` and `height_m`. Resolution, chunk size, whether the
    /// candidate library is attached, and the seed are taken from this object in
    /// every case, so two requests that differ only in `seed` produce two
    /// different maps and the same request reproduces the same one.
    pub fn to_spec(&self) -> ourealis_map_format::synthetic::SyntheticMapSpec {
        use ourealis_map_format::synthetic::SyntheticMapSpec;
        let mut spec = match self.preset.as_str() {
            "compact" => SyntheticMapSpec::compact(),
            "default" | "" => SyntheticMapSpec::default(),
            _ => SyntheticMapSpec {
                width_m: self.width_m.max(50.0),
                height_m: self.height_m.max(50.0),
                ..SyntheticMapSpec::default()
            },
        };
        spec.resolution_m = self.resolution_m.max(0.1);
        spec.chunk_size = self.chunk_size.max(8);
        spec.with_kpath_library = self.with_kpath_library;
        spec.seed = self.seed;
        spec
    }
}

/// How the route is specified.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum RouteSpec {
    /// Start, optional waypoints, goal.
    Standard {
        /// Start position in the local plane.
        start: Vec2,
        /// Goal position in the local plane.
        goal: Vec2,
        /// Ordered waypoints.
        #[serde(default)]
        waypoints: Vec<WaypointSpec>,
    },
    /// A closed circuit through a reference point.
    Loop {
        /// Start and finish position.
        start: Vec2,
        /// Reference point the circuit passes through, or `null` to let the
        /// planner choose the far side of the map.
        #[serde(default)]
        reference: Option<Vec2>,
        /// Number of laps laid end to end.
        #[serde(default = "default_laps")]
        laps: usize,
    },
    /// A run that is redirected by checkpoints as it goes.
    Dynamic {
        /// Start position.
        start: Vec2,
        /// Goal position.
        goal: Vec2,
        /// Checkpoints with the time each takes effect.
        checkpoints: Vec<CheckpointSpec>,
    },
}

fn default_laps() -> usize {
    1
}

/// One waypoint of a standard route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaypointSpec {
    /// Position in the local plane.
    pub position: Vec2,
    /// Passing behaviour.
    #[serde(default)]
    pub semantics: SemanticsSpec,
    /// Radius the `slow` behaviour applies over, metres.
    #[serde(default = "default_waypoint_radius")]
    pub radius_m: f64,
}

fn default_waypoint_radius() -> f64 {
    5.0
}

/// Passing behaviour of a waypoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SemanticsSpec {
    /// Run through without slowing.
    #[default]
    Pass,
    /// Slow to a fraction of the local limit inside the radius.
    Slow,
    /// Stand still for the given time.
    Dwell {
        /// Standing time, seconds.
        duration_s: f64,
    },
}

impl From<SemanticsSpec> for ViaSemantics {
    fn from(value: SemanticsSpec) -> Self {
        match value {
            SemanticsSpec::Pass => ViaSemantics::Pass,
            SemanticsSpec::Slow => ViaSemantics::Slow,
            SemanticsSpec::Dwell { duration_s } => ViaSemantics::Dwell { duration_s },
        }
    }
}

/// One checkpoint of a dynamic route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointSpec {
    /// Position the runner is redirected to.
    pub position: Vec2,
    /// Time the checkpoint takes effect, seconds from the start.
    pub issued_at_s: f64,
}

/// The individual to simulate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PersonSpec {
    /// Preset the parameters start from.
    pub preset: String,
    /// Field-level overrides applied on top of the preset.
    pub overrides: PersonOverrides,
}

impl Default for PersonSpec {
    fn default() -> Self {
        Self {
            preset: "moderate".to_string(),
            overrides: PersonOverrides::default(),
        }
    }
}

impl PersonSpec {
    /// Resolves the individual.
    pub fn resolve(&self) -> Result<PersonParams> {
        let preset = match self.preset.to_ascii_lowercase().as_str() {
            "jog" => Preset::Jog,
            "moderate" | "" => Preset::Moderate,
            "race" => Preset::Race,
            other => {
                return Err(ServiceError::Invalid(format!(
                    "unknown preset {other:?}; expected one of jog, moderate, race"
                )));
            }
        };
        let mut person = PersonParams::preset(preset);
        self.overrides.apply(&mut person);
        person
            .validate()
            .map_err(|error| ServiceError::Invalid(format!("individual parameters: {error}")))?;
        Ok(person)
    }
}

/// Field-level overrides of an individual's parameters.
///
/// Generated from a field list so the wire names cannot drift from [`PersonParams`].
/// Every field is optional; a field left out keeps the preset's value. An unknown
/// name is rejected with the list of accepted ones, which is what makes the
/// request self-documenting.
macro_rules! person_overrides {
    ($( $field:ident : $ty:ty ),* $(,)?) => {
        /// Overrides of individual parameters.
        #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        pub struct PersonOverrides {
            $(
                #[doc = concat!("Override for `PersonParams::", stringify!($field), "`.")]
                pub $field: Option<$ty>,
            )*
        }

        impl PersonOverrides {
            /// Applies the set fields to a parameter vector.
            pub fn apply(&self, person: &mut PersonParams) {
                $(
                    if let Some(value) = self.$field.clone() {
                        person.$field = value.into();
                    }
                )*
            }

            /// Reads every field out of a parameter vector.
            pub fn from_person(person: &PersonParams) -> Self {
                Self {
                    $( $field: Some(person.$field.clone().into()), )*
                }
            }

            /// Names of the fields this type accepts.
            pub const FIELDS: &'static [&'static str] = &[$( stringify!($field) ),*];
        }
    };
}

person_overrides! {
    label: Option<String>,
    target_speed: f64,
    step_frequency: f64,
    a_max: f64,
    a_lat_max: f64,
    look_ahead_m: f64,
    beta_logit: f64,
    lateral_offset_mean: f64,
    lateral_offset_std: f64,
    lateral_offset_tau_s: f64,
    pace_drift_sigma: f64,
    pace_drift_tau_s: f64,
    critical_speed_ratio: f64,
    fatigue_tau_s: f64,
    pace_strategy: PaceStrategyDto,
    split_amplitude: f64,
    k_down: f64,
    lean_max_deg: f64,
    bounce_amplitude_m: f64,
    head_look_ahead_s: f64,
    turn_omega_max: f64,
}

/// Pace distribution strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaceStrategyDto {
    /// Constant target speed.
    Even,
    /// Start fast, fade.
    PositiveSplit,
    /// Start steady, finish faster.
    NegativeSplit,
}

impl From<PaceStrategy> for PaceStrategyDto {
    fn from(value: PaceStrategy) -> Self {
        match value {
            PaceStrategy::Even => PaceStrategyDto::Even,
            PaceStrategy::PositiveSplit => PaceStrategyDto::PositiveSplit,
            PaceStrategy::NegativeSplit => PaceStrategyDto::NegativeSplit,
        }
    }
}

impl From<PaceStrategyDto> for PaceStrategy {
    fn from(value: PaceStrategyDto) -> Self {
        match value {
            PaceStrategyDto::Even => PaceStrategy::Even,
            PaceStrategyDto::PositiveSplit => PaceStrategy::PositiveSplit,
            PaceStrategyDto::NegativeSplit => PaceStrategy::NegativeSplit,
        }
    }
}

/// A simulation request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulationRequest {
    /// Optional name for the job.
    #[serde(default)]
    pub name: Option<String>,
    /// Map to run on. Absent means the library must hold exactly one map: with
    /// several the service refuses rather than pick one, because the choice changes
    /// the result and a silent default would hide that.
    #[serde(default)]
    pub map: Option<MapRef>,
    /// Route to run.
    pub route: RouteSpec,
    /// Individual to simulate.
    #[serde(default)]
    pub person: PersonSpec,
    /// Seed of the run's random streams.
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// Index of the individual inside a batch, which selects the noise streams.
    #[serde(default)]
    pub individual: u32,
    /// Simulator settings; every field is optional.
    #[serde(default)]
    pub settings: SimulationSettings,
}

fn default_seed() -> u64 {
    0x5EED_1234
}

impl SimulationRequest {
    /// Drops the bytes of an inline map image.
    ///
    /// Called when a job reaches a terminal state: the parameters stay for reporting,
    /// the payload — which may be as large as the upload limit — does not.
    pub fn release_payload(&mut self) {
        if let Some(MapRef::Inline { omf_base64, .. }) = &mut self.map {
            omf_base64.clear();
            omf_base64.shrink_to_fit();
        }
    }

    /// The standard route as the planner's own request type.
    pub fn standard_request(&self) -> Result<StandardRequest> {
        match &self.route {
            RouteSpec::Standard {
                start,
                goal,
                waypoints,
            } => {
                let mut request = StandardRequest::new((*start).into(), (*goal).into());
                request.waypoints = waypoints
                    .iter()
                    .map(|waypoint| {
                        let mut converted = Waypoint::new(waypoint.position.into());
                        converted.semantics = waypoint.semantics.into();
                        converted.radius_m = waypoint.radius_m.max(0.0);
                        converted
                    })
                    .collect();
                Ok(request)
            }
            other => Err(ServiceError::Invalid(format!(
                "this endpoint needs a standard route, got {}",
                other.mode_name()
            ))),
        }
    }

    /// The loop route as the planner's own request type.
    pub fn loop_request(&self) -> Result<LoopRequest> {
        match &self.route {
            RouteSpec::Loop {
                start,
                reference,
                laps,
            } => {
                let mut request = LoopRequest::new((*start).into(), (*laps).max(1));
                request.reference = reference.map(Into::into);
                Ok(request)
            }
            other => Err(ServiceError::Invalid(format!(
                "this endpoint needs a loop route, got {}",
                other.mode_name()
            ))),
        }
    }

    /// The dynamic route as the planner's own request type, with its checkpoints.
    pub fn dynamic_request(
        &self,
    ) -> Result<(StandardRequest, Vec<ourealis_core::plan::Checkpoint>)> {
        match &self.route {
            RouteSpec::Dynamic {
                start,
                goal,
                checkpoints,
            } => {
                let request = StandardRequest::new((*start).into(), (*goal).into());
                let checkpoints = checkpoints
                    .iter()
                    .map(|checkpoint| ourealis_core::plan::Checkpoint {
                        position: checkpoint.position.into(),
                        issued_at_s: checkpoint.issued_at_s.max(0.0),
                    })
                    .collect();
                Ok((request, checkpoints))
            }
            other => Err(ServiceError::Invalid(format!(
                "this endpoint needs a dynamic route, got {}",
                other.mode_name()
            ))),
        }
    }

    /// Mode name used in listings and error messages.
    pub fn mode_name(&self) -> &'static str {
        self.route.mode_name()
    }
}

impl RouteSpec {
    /// Mode name of this route specification.
    pub fn mode_name(&self) -> &'static str {
        match self {
            RouteSpec::Standard { .. } => "standard",
            RouteSpec::Loop { .. } => "loop",
            RouteSpec::Dynamic { .. } => "dynamic",
        }
    }
}

/// Simulator settings a request may override.
///
/// A curated subset of [`SimulationConfig`]: the knobs a client can reasonably be
/// expected to set. Anything left out keeps the simulator's own default, so a
/// request that only names a route is complete.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SimulationSettings {
    /// Motion mode whose cost weights are used.
    pub mode: Option<MotionModeDto>,
    /// Explicit cost weights, overriding the stored prior.
    pub weight_override: Option<Vec<f64>>,
    /// Attention gate applied on top of the prior, when a context is given.
    pub attention: Option<AttentionSettings>,
    /// Compute backend.
    pub backend: Option<BackendDto>,
    /// Whether skeleton leaves join the search graph.
    pub coarse_enabled: Option<bool>,
    /// Largest admitted coarse block side, metres.
    pub coarse_max_size_m: Option<f64>,
    /// Roadmap handling.
    pub roadmap: Option<RoadmapSettings>,
    /// Route planning.
    pub route: RouteSettings,
    /// Motion generation.
    pub motion: MotionSettings,
    /// Lateral offset habit.
    pub offset: OffsetSettings,
    /// Speed limits.
    pub limits: LimitSettings,
    /// Sensor simulation.
    pub sensors: Option<SensorSettings>,
    /// Evaluate the metrics report.
    pub with_metrics: Option<bool>,
}

/// Motion mode names as they appear on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionModeDto {
    /// Easy running.
    Jog,
    /// Steady training pace.
    Moderate,
    /// Racing pace.
    Race,
}

impl From<MotionModeDto> for MotionMode {
    fn from(value: MotionModeDto) -> Self {
        match value {
            MotionModeDto::Jog => MotionMode::Jog,
            MotionModeDto::Moderate => MotionMode::Moderate,
            MotionModeDto::Race => MotionMode::Race,
        }
    }
}

/// Compute backend names as they appear on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendDto {
    /// Use the GPU when an adapter exists.
    Auto,
    /// Force the CPU implementation.
    Cpu,
    /// Require the GPU.
    Gpu,
}

impl From<BackendDto> for Backend {
    fn from(value: BackendDto) -> Self {
        match value {
            BackendDto::Auto => Backend::Auto,
            BackendDto::Cpu => Backend::Cpu,
            BackendDto::Gpu => Backend::Gpu,
        }
    }
}

/// Attention gate settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AttentionSettings {
    /// Context vector fed to the gate.
    pub context: Vec<f64>,
    /// Cap on the modulation factor.
    pub clip_min: f64,
    /// Floor on the modulation factor.
    pub clip_max: f64,
    /// Query projection, row major.
    pub w_q: Vec<f64>,
    /// Key projection, row major.
    pub w_k: Vec<f64>,
}

impl Default for AttentionSettings {
    fn default() -> Self {
        Self {
            context: Vec::new(),
            clip_min: 0.2,
            clip_max: 5.0,
            w_q: Vec::new(),
            w_k: Vec::new(),
        }
    }
}

/// Roadmap handling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoadmapSettings {
    /// Search the fine grid only.
    None,
    /// Load the batch stored in the map.
    Stored {
        /// Batch index.
        #[serde(default)]
        batch: u16,
    },
    /// Sample a fresh roadmap.
    Generate {
        /// Sampling seed.
        seed: u64,
        /// Waypoint spacing, metres.
        spacing_m: f64,
        /// Longest edge the sampler may create, metres.
        connect_radius_m: f64,
    },
}

/// Route planning settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RouteSettings {
    /// Heuristic inflation for multi-leg requests.
    pub epsilon: Option<f64>,
    /// Heuristic inflation for a single leg.
    pub single_leg_epsilon: Option<f64>,
    /// Weight of the turn penalty, in equivalent metres.
    pub turn_penalty: Option<f64>,
    /// Node expansion budget per search.
    pub max_expansions: Option<usize>,
    /// Number of candidate paths kept per leg.
    pub candidates_k: Option<usize>,
    /// Multiplier applied to the edges of an accepted candidate.
    pub penalty_mu: Option<f64>,
    /// Largest shared fraction of two candidates before one is rejected.
    pub max_overlap_ratio: Option<f64>,
    /// Longest attach segment a stored candidate path may need, metres.
    pub d_attach_m: Option<f64>,
    /// Corner rounding radius, metres.
    pub corner_radius_m: Option<f64>,
    /// Whether smoothing runs.
    pub smooth: Option<bool>,
    /// Arc-length spacing of the smoothed path, metres.
    pub sample_spacing_m: Option<f64>,
    /// Elastic band iterations.
    pub smoothing_iterations: Option<usize>,
}

/// Motion generation settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MotionSettings {
    /// Sample rate of the truth timeline, Hz.
    pub sample_rate_hz: Option<f64>,
    /// Standing time before the run, seconds.
    pub start_stand_s: Option<f64>,
    /// Standing time after the run, seconds.
    pub end_stand_s: Option<f64>,
    /// Whether maneuvers are scheduled at all.
    pub maneuvers_enabled: Option<bool>,
    /// Arc-length spacing of the profile samples, metres.
    pub profile_spacing_m: Option<f64>,
    /// Second harmonic of the bounce waveform.
    pub bounce_beta2: Option<f64>,
    /// Head look-ahead used by the gyroscope, seconds.
    pub head_look_ahead_s: Option<f64>,
    /// Angular acceleration of an on-the-spot turn, rad/s^2.
    pub turn_alpha: Option<f64>,
}

/// Lateral offset settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OffsetSettings {
    /// Mean offset, metres; positive is the runner's left.
    pub mean_m: Option<f64>,
    /// Stationary standard deviation of the offset, metres.
    pub std_m: Option<f64>,
    /// Relaxation time of the offset process, seconds.
    pub tau_s: Option<f64>,
    /// Low-pass time constant applied to the offset, seconds.
    pub smoothing_s: Option<f64>,
    /// Lateral acceleration budget used to limit the offset, m/s^2.
    pub a_lat_max: Option<f64>,
}

/// Speed limit settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LimitSettings {
    /// Grade look-ahead distance, metres.
    pub look_ahead_m: Option<f64>,
    /// How the look-ahead window is reduced to one grade.
    pub look_ahead_mode: Option<LookAheadModeDto>,
    /// Lateral acceleration limit used for the curvature cap, m/s^2.
    pub a_lat_max: Option<f64>,
    /// Coefficient of the downhill speed cap.
    pub k_down: Option<f64>,
    /// Grade clamp applied to the metabolic cost model.
    pub grade_clamp: Option<f64>,
}

/// Look-ahead aggregation modes as they appear on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LookAheadModeDto {
    /// Distance-weighted mean over the window.
    DistanceWeightedMean,
    /// Largest uphill grade in the window.
    WorstCase,
}

impl From<LookAheadModeDto> for LookAheadMode {
    fn from(value: LookAheadModeDto) -> Self {
        match value {
            LookAheadModeDto::DistanceWeightedMean => LookAheadMode::DistanceWeightedMean,
            LookAheadModeDto::WorstCase => LookAheadMode::WorstCase,
        }
    }
}

/// Sensor simulation settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SensorSettings {
    /// GNSS fix rate, Hz.
    pub gnss_rate_hz: Option<f64>,
    /// Inertial rate, Hz.
    pub imu_rate_hz: Option<f64>,
    /// Magnetometer rate, Hz.
    pub mag_rate_hz: Option<f64>,
    /// Barometer rate, Hz.
    pub baro_rate_hz: Option<f64>,
    /// Whether multipath events fire.
    pub multipath_enabled: Option<bool>,
    /// Whether magnetic disturbances fire.
    pub magnetic_disturbance_enabled: Option<bool>,
    /// Whether the position jitter layer runs.
    pub jitter_enabled: Option<bool>,
    /// Position jitter standard deviation, metres.
    pub jitter_sigma_m: Option<f64>,
    /// Where the inertial unit is carried.
    pub mount: Option<DeviceMountDto>,
    /// Force the deterministic event mode the design requires for calibration and
    /// regression runs, whatever the map declares.
    pub force_deterministic_events: Option<bool>,
    /// Pressure at mean sea level used by the barometer, pascals.
    pub reference_pressure_pa: Option<f64>,
}

/// Device mounts as they appear on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceMountDto {
    /// Carried on the torso.
    Body,
    /// Worn on the head.
    Head,
}

impl From<DeviceMountDto> for DeviceMount {
    fn from(value: DeviceMountDto) -> Self {
        match value {
            DeviceMountDto::Body => DeviceMount::Body,
            DeviceMountDto::Head => DeviceMount::Head,
        }
    }
}

impl SimulationSettings {
    /// Applies the settings to a simulator configuration.
    ///
    /// Only the fields a client set are touched; everything else keeps whatever
    /// the configuration already held.
    pub fn apply(&self, config: &mut SimulationConfig) -> Result<()> {
        if let Some(mode) = self.mode {
            config.mode = mode.into();
        }
        if let Some(weights) = &self.weight_override {
            if weights
                .iter()
                .any(|weight| !weight.is_finite() || *weight < 0.0)
            {
                return Err(ServiceError::Invalid(
                    "weight_override entries must be finite and non-negative".to_string(),
                ));
            }
            config.weight_override = Some(weights.clone());
        }
        if let Some(attention) = &self.attention {
            let dimension = attention.clip_bounds()?;
            config.attention = Some(dimension);
            config.attention_context = attention.context.clone();
        }
        if let Some(backend) = self.backend {
            config.backend = backend.into();
        }
        if let Some(enabled) = self.coarse_enabled {
            config.coarse.enabled = enabled;
        }
        if let Some(size) = self.coarse_max_size_m {
            if size <= 0.0 {
                return Err(ServiceError::Invalid(
                    "coarse_max_size_m must be positive".to_string(),
                ));
            }
            config.coarse.max_size_m = size;
        }
        if let Some(roadmap) = &self.roadmap {
            config.prm = roadmap.to_options()?;
        }
        self.route.apply(&mut config.route)?;
        self.motion.apply(&mut config.motion)?;
        self.offset.apply(&mut config.motion.offset)?;
        self.limits.apply(&mut config.motion.limits);
        if let Some(sensors) = &self.sensors {
            sensors.apply(&mut config.sensors)?;
        }
        if let Some(with_metrics) = self.with_metrics {
            config.with_metrics = with_metrics;
        }
        Ok(())
    }
}

impl AttentionSettings {
    /// Validates the gate matrices and builds the gate.
    fn clip_bounds(&self) -> Result<ourealis_core::field::AttentionGating> {
        let embedding_dim = if self.context.is_empty() {
            return Err(ServiceError::Invalid(
                "attention.context must carry at least one value".to_string(),
            ));
        } else {
            self.w_q
                .len()
                .checked_div(self.context.len())
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    ServiceError::Invalid(
                        "attention.w_q must have a whole number of rows per context value"
                            .to_string(),
                    )
                })?
        };
        if self.w_q.len() != embedding_dim * self.context.len() {
            return Err(ServiceError::Invalid(
                "attention.w_q length must be embedding_dim * context length".to_string(),
            ));
        }
        if self.clip_min <= 0.0 || self.clip_max < self.clip_min {
            return Err(ServiceError::Invalid(
                "attention clip bounds must satisfy 0 < clip_min <= clip_max".to_string(),
            ));
        }
        Ok(ourealis_core::field::AttentionGating {
            embedding_dim,
            w_q: self.w_q.clone(),
            w_k: self.w_k.clone(),
            clip_min: self.clip_min,
            clip_max: self.clip_max,
        })
    }
}

impl RoadmapSettings {
    /// Builds the roadmap option.
    fn to_options(&self) -> Result<ourealis_core::PrmOptions> {
        Ok(match self {
            RoadmapSettings::None => ourealis_core::PrmOptions::None,
            RoadmapSettings::Stored { batch } => ourealis_core::PrmOptions::Stored(*batch),
            RoadmapSettings::Generate {
                seed,
                spacing_m,
                connect_radius_m,
            } => {
                if *spacing_m <= 0.0 || *connect_radius_m <= 0.0 {
                    return Err(ServiceError::Invalid(
                        "roadmap spacing and connect radius must be positive".to_string(),
                    ));
                }
                ourealis_core::PrmOptions::Generate {
                    seed: *seed,
                    params: ourealis_core::graph::prm::PrmParams {
                        spacing_m: *spacing_m,
                        connect_radius_m: *connect_radius_m,
                        ..Default::default()
                    },
                }
            }
        })
    }
}

impl RouteSettings {
    /// Applies the route settings.
    fn apply(&self, route: &mut ourealis_core::plan::RouteConfig) -> Result<()> {
        let positive = |name: &str, value: Option<f64>| -> Result<()> {
            match value {
                Some(value) if value <= 0.0 => {
                    Err(ServiceError::Invalid(format!("{name} must be positive")))
                }
                _ => Ok(()),
            }
        };
        positive("route.epsilon", self.epsilon)?;
        positive("route.single_leg_epsilon", self.single_leg_epsilon)?;
        positive("route.penalty_mu", self.penalty_mu)?;
        positive("route.d_attach_m", self.d_attach_m)?;
        positive("route.corner_radius_m", self.corner_radius_m)?;

        if let Some(value) = self.epsilon {
            route.search.epsilon = value;
        }
        if let Some(value) = self.single_leg_epsilon {
            route.single_leg_epsilon = value;
        }
        if let Some(value) = self.turn_penalty {
            if value < 0.0 {
                return Err(ServiceError::Invalid(
                    "route.turn_penalty must not be negative".to_string(),
                ));
            }
            route.search.turn_penalty = value;
        }
        if let Some(value) = self.max_expansions {
            route.search.max_expansions = value.max(1);
        }
        if let Some(value) = self.candidates_k {
            route.candidates.k = value.max(1);
        }
        if let Some(value) = self.penalty_mu {
            route.candidates.penalty_mu = value;
        }
        if let Some(value) = self.max_overlap_ratio {
            if !(0.0..=1.0).contains(&value) {
                return Err(ServiceError::Invalid(
                    "route.max_overlap_ratio must lie in [0, 1]".to_string(),
                ));
            }
            route.candidates.max_overlap_ratio = value;
        }
        if let Some(value) = self.d_attach_m {
            route.d_attach_m = value;
        }
        if let Some(value) = self.corner_radius_m {
            route.corner_radius_m = value;
        }
        if let Some(value) = self.smooth {
            route.smooth = value;
        }
        if let Some(value) = self.sample_spacing_m {
            route.sample_spacing_m = value;
        }
        if let Some(value) = self.smoothing_iterations {
            route.smoothing.iterations = value.max(1);
        }
        Ok(())
    }
}

impl MotionSettings {
    /// Applies the motion settings.
    fn apply(&self, motion: &mut MotionConfig) -> Result<()> {
        if let Some(rate) = self.sample_rate_hz
            && rate <= 0.0
        {
            return Err(ServiceError::Invalid(
                "motion.sample_rate_hz must be positive".to_string(),
            ));
        }
        if let Some(rate) = self.sample_rate_hz {
            motion.sample_rate_hz = rate;
        }
        if let Some(value) = self.start_stand_s {
            motion.maneuver.start_stand_s = value.max(0.0);
        }
        if let Some(value) = self.end_stand_s {
            motion.maneuver.end_stand_s = value.max(0.0);
        }
        if let Some(value) = self.maneuvers_enabled {
            motion.maneuver.enabled = value;
        }
        if let Some(value) = self.profile_spacing_m {
            motion.profile.sample_spacing_m = value.max(0.05);
        }
        if let Some(value) = self.bounce_beta2 {
            motion.bounce_beta2 = value;
        }
        if let Some(value) = self.head_look_ahead_s {
            motion.attitude.head_look_ahead_s = value.max(0.0);
        }
        if let Some(value) = self.turn_alpha {
            if value <= 0.0 {
                return Err(ServiceError::Invalid(
                    "motion.turn_alpha must be positive".to_string(),
                ));
            }
            motion.maneuver.turn_alpha = value;
        }
        Ok(())
    }
}

impl OffsetSettings {
    /// Applies the offset settings.
    fn apply(&self, offset: &mut ourealis_core::motion::OffsetConfig) -> Result<()> {
        if let Some(value) = self.std_m
            && value < 0.0
        {
            return Err(ServiceError::Invalid(
                "offset.std_m must not be negative".to_string(),
            ));
        }
        if let Some(value) = self.tau_s
            && value <= 0.0
        {
            return Err(ServiceError::Invalid(
                "offset.tau_s must be positive".to_string(),
            ));
        }
        if let Some(value) = self.mean_m {
            offset.mean_m = value;
        }
        if let Some(value) = self.std_m {
            offset.std_m = value;
        }
        if let Some(value) = self.tau_s {
            offset.tau_s = value;
        }
        if let Some(value) = self.smoothing_s {
            offset.smoothing_s = value.max(0.0);
        }
        if let Some(value) = self.a_lat_max {
            if value <= 0.0 {
                return Err(ServiceError::Invalid(
                    "offset.a_lat_max must be positive".to_string(),
                ));
            }
            offset.a_lat_max = value;
        }
        Ok(())
    }
}

impl LimitSettings {
    /// Applies the limit settings.
    fn apply(&self, limits: &mut ourealis_core::motion::SpeedLimitParams) {
        if let Some(value) = self.look_ahead_m {
            limits.look_ahead_m = value.max(0.0);
        }
        if let Some(value) = self.look_ahead_mode {
            limits.look_ahead_mode = value.into();
        }
        if let Some(value) = self.a_lat_max {
            limits.a_lat_max = value;
        }
        if let Some(value) = self.k_down {
            limits.k_down = value;
        }
        if let Some(value) = self.grade_clamp {
            limits.grade_clamp = value;
        }
    }
}

impl SensorSettings {
    /// Applies the sensor settings.
    fn apply(&self, sensors: &mut ourealis_core::sensor::SensorConfig) -> Result<()> {
        for (name, rate) in [
            ("gnss_rate_hz", self.gnss_rate_hz),
            ("imu_rate_hz", self.imu_rate_hz),
            ("mag_rate_hz", self.mag_rate_hz),
            ("baro_rate_hz", self.baro_rate_hz),
        ] {
            if let Some(rate) = rate
                && rate <= 0.0
            {
                return Err(ServiceError::Invalid(format!(
                    "sensors.{name} must be positive"
                )));
            }
        }
        if let Some(value) = self.gnss_rate_hz {
            sensors.gnss_rate_hz = value;
        }
        if let Some(value) = self.imu_rate_hz {
            sensors.imu_rate_hz = value;
        }
        if let Some(value) = self.mag_rate_hz {
            sensors.mag_rate_hz = value;
        }
        if let Some(value) = self.baro_rate_hz {
            sensors.baro_rate_hz = value;
        }
        if let Some(value) = self.multipath_enabled {
            sensors.multipath_enabled = value;
        }
        if let Some(value) = self.magnetic_disturbance_enabled {
            sensors.magnetic_disturbance_enabled = value;
        }
        if let Some(value) = self.jitter_enabled {
            sensors.jitter_enabled = value;
        }
        if let Some(value) = self.jitter_sigma_m {
            sensors.jitter_sigma_m = value.max(0.0);
        }
        if let Some(value) = self.mount {
            sensors.mount = value.into();
        }
        if let Some(value) = self.force_deterministic_events {
            sensors.force_deterministic_events = value;
        }
        if let Some(value) = self.reference_pressure_pa {
            sensors.reference_pressure_pa = value;
        }
        sensors
            .validate()
            .map_err(|error| ServiceError::Invalid(format!("sensor settings: {error}")))?;
        Ok(())
    }
}
