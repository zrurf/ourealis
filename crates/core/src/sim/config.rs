//! Simulation configuration.

use ourealis_map_format::MotionMode;
use serde::{Deserialize, Serialize};

use crate::environment::PrmOptions;
use crate::field::{AttentionGating, CostModelParams};
use crate::motion::MotionConfig;
use crate::plan::{DynamicConfig, LoopConfig, RouteConfig};
use crate::sensor::SensorConfig;

/// Which compute backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum Backend {
    /// Use the GPU when an adapter is available, otherwise the CPU.
    #[default]
    Auto = 0,
    /// Force the CPU implementation; used by tests and for reference runs.
    Cpu = 1,
    /// Require the GPU; failing to obtain an adapter is an error.
    Gpu = 2,
}

/// Everything that shapes a simulation run.
#[derive(Debug, Clone, PartialEq)]
pub struct SimulationConfig {
    /// Cost model parameters.
    pub cost: CostModelParams,
    /// Motion mode whose weight prior is used.
    pub mode: MotionMode,
    /// Optional explicit weight vector, overriding the stored prior.
    pub weight_override: Option<Vec<f64>>,
    /// Optional attention gate applied on top of the prior.
    pub attention: Option<AttentionGating>,
    /// Context vector fed to the attention gate.
    pub attention_context: Vec<f64>,
    /// Coarse granularity, taken from the map's quadtree skeleton.
    pub coarse: crate::graph::CoarseOptions,
    /// Roadmap configuration.
    pub prm: PrmOptions,
    /// Route planning configuration.
    pub route: RouteConfig,
    /// Loop planning configuration.
    pub loop_route: LoopConfig,
    /// Dynamic re-planning configuration.
    pub dynamic: DynamicConfig,
    /// Motion configuration.
    pub motion: MotionConfig,
    /// Sensor configuration.
    pub sensors: SensorConfig,
    /// Whether the evaluation report is computed.
    pub with_metrics: bool,
    /// Compute backend.
    pub backend: Backend,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            cost: CostModelParams::default(),
            mode: MotionMode::Moderate,
            weight_override: None,
            attention: None,
            attention_context: Vec::new(),
            coarse: crate::graph::CoarseOptions::default(),
            prm: PrmOptions::default(),
            route: RouteConfig::default(),
            loop_route: LoopConfig::default(),
            dynamic: DynamicConfig::default(),
            motion: MotionConfig::default(),
            sensors: SensorConfig::default(),
            with_metrics: true,
            backend: Backend::Auto,
        }
    }
}

impl SimulationConfig {
    /// A configuration with sensor noise disabled and no metrics, for tests.
    pub fn deterministic() -> Self {
        Self {
            sensors: SensorConfig {
                force_deterministic_events: true,
                ..SensorConfig::clean()
            },
            with_metrics: true,
            backend: Backend::Cpu,
            ..Default::default()
        }
    }

    /// Keeps the truth sample rate equal to the inertial rate.
    ///
    /// The inertial sensors read the truth sequence directly, so a mismatch would
    /// either alias the step signature or duplicate samples.
    pub fn align_rates(&mut self) {
        self.motion.sample_rate_hz = self.sensors.imu_rate_hz;
    }
}
