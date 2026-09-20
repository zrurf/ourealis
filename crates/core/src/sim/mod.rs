//! Simulator facade.
//!
//! ```no_run
//! use glam::DVec2;
//! use ourealis_core::person::{PersonParams, Preset};
//! use ourealis_core::plan::StandardRequest;
//! use ourealis_core::sim::{MapSource, Simulator};
//!
//! # fn main() -> ourealis_core::Result<()> {
//! let output = Simulator::builder()
//!     .map(MapSource::omf("campus.omf"))
//!     .person(PersonParams::preset(Preset::Moderate))
//!     .standard(StandardRequest::new(DVec2::new(10.0, 10.0), DVec2::new(300.0, 200.0)))
//!     .seed(42)
//!     .build()?
//!     .run()?;
//! println!("{:.1} s, {} fixes", output.duration_s(), output.sensors.gnss.len());
//! # Ok(())
//! # }
//! ```
//!
//! The API is synchronous on purpose: the work is CPU-bound and already
//! parallel inside a batch, so an async surface would add cost without adding
//! capability. Callers that need to run it off a runtime can wrap it in a
//! blocking task.

pub mod config;
pub mod connector_lift;
pub mod export;
pub mod output;

use std::path::PathBuf;

use rayon::prelude::*;

use ourealis_map_format::Map;
use ourealis_map_format::synthetic::{self, SyntheticMapSpec};

use crate::environment::Environment;
use crate::error::{CoreError, Result};
use crate::eval::MetricsReport;
use crate::field::CostWeights;
use crate::graph::MixedGraph;
use crate::motion::Trajectory;
use crate::person::{PersonParams, Preset};
use crate::plan::{LoopRequest, PlanMode, PlannedRoute, StandardRequest, dynamic, loop_mode, plan};
use crate::sensor;
use crate::smooth::ElasticBandConfig;

pub use config::{Backend, SimulationConfig};
pub use output::{CandidateSummary, RouteSummary, RunManifest, SimulationOutput};

/// Where the environment comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum MapSource {
    /// An OMF file on disk.
    Omf(PathBuf),
    /// An OMF image already in memory.
    Bytes(Vec<u8>),
    /// A deterministically generated map, useful for examples and tests.
    Synthetic(Box<SyntheticMapSpec>),
}

impl MapSource {
    /// An OMF file on disk.
    pub fn omf(path: impl Into<PathBuf>) -> Self {
        MapSource::Omf(path.into())
    }

    /// An in-memory OMF image.
    pub fn bytes(image: Vec<u8>) -> Self {
        MapSource::Bytes(image)
    }

    /// A generated map.
    pub fn synthetic(spec: SyntheticMapSpec) -> Self {
        MapSource::Synthetic(Box::new(spec))
    }

    /// Opens the map.
    pub fn open(&self) -> Result<Map> {
        match self {
            MapSource::Omf(path) => Ok(Map::open(path)?),
            MapSource::Bytes(image) => Ok(Map::from_bytes(image.clone())?),
            MapSource::Synthetic(spec) => Ok(Map::from_bytes(synthetic::build(spec)?)?),
        }
    }
}

/// Builds a [`Simulator`].
#[derive(Debug, Clone)]
pub struct SimulatorBuilder {
    source: Option<MapSource>,
    person: PersonParams,
    mode: Option<PlanMode>,
    config: SimulationConfig,
    seed: u64,
    individual: u32,
}

impl Default for SimulatorBuilder {
    fn default() -> Self {
        Self {
            source: None,
            person: PersonParams::preset(Preset::Moderate),
            mode: None,
            config: SimulationConfig::default(),
            seed: 1,
            individual: 0,
        }
    }
}

impl SimulatorBuilder {
    /// Sets the map source.
    pub fn map(mut self, source: MapSource) -> Self {
        self.source = Some(source);
        self
    }

    /// Sets the individual.
    pub fn person(mut self, person: PersonParams) -> Self {
        self.person = person;
        self
    }

    /// Uses a standard route.
    pub fn standard(mut self, request: StandardRequest) -> Self {
        self.mode = Some(PlanMode::Standard(request));
        self
    }

    /// Uses a closed loop.
    pub fn looped(mut self, request: LoopRequest) -> Self {
        self.mode = Some(PlanMode::Loop(request));
        self
    }

    /// Uses a route that may be redirected by checkpoints.
    pub fn dynamic(
        mut self,
        request: StandardRequest,
        checkpoints: Vec<crate::plan::Checkpoint>,
    ) -> Self {
        self.mode = Some(PlanMode::Dynamic {
            request,
            checkpoints,
        });
        self
    }

    /// Sets the global seed.
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// Sets the individual index, which selects the random streams.
    pub fn individual(mut self, index: u32) -> Self {
        self.individual = index;
        self
    }

    /// Replaces the configuration.
    pub fn config(mut self, config: SimulationConfig) -> Self {
        self.config = config;
        self
    }

    /// Sets the configuration and the global seed together.
    pub fn setup(mut self, config: SimulationConfig, seed: u64) -> Self {
        self.config = config;
        self.seed = seed;
        self
    }

    /// Enables or disables the evaluation report.
    pub fn with_metrics(mut self, enabled: bool) -> Self {
        self.config.with_metrics = enabled;
        self
    }

    /// Builds the simulator.
    pub fn build(mut self) -> Result<Simulator> {
        let source = self
            .source
            .ok_or_else(|| CoreError::config("no map source was provided"))?;
        let mode = self
            .mode
            .ok_or_else(|| CoreError::config("no planning mode was provided"))?;
        self.person.validate()?;
        self.config.align_rates();
        Ok(Simulator {
            source,
            person: self.person,
            mode,
            config: self.config,
            seed: self.seed,
            individual: self.individual,
        })
    }
}

/// A configured, ready-to-run simulation.
#[derive(Debug, Clone)]
pub struct Simulator {
    source: MapSource,
    person: PersonParams,
    mode: PlanMode,
    config: SimulationConfig,
    seed: u64,
    individual: u32,
}

impl Simulator {
    /// Starts building a simulation.
    pub fn builder() -> SimulatorBuilder {
        SimulatorBuilder::default()
    }

    /// Configuration in use.
    pub fn config(&self) -> &SimulationConfig {
        &self.config
    }

    /// Individual in use.
    pub fn person(&self) -> &PersonParams {
        &self.person
    }

    /// Loads the environment of this simulation.
    pub fn environment(&self) -> Result<Environment> {
        let map = self.source.open()?;
        let weights = self.weights(&map)?;
        let backend = self.compute_backend()?;
        Environment::load_with_coarse(
            &map,
            &weights,
            &self.config.cost,
            self.config.prm,
            self.config.coarse,
            backend.as_deref(),
        )
    }

    /// Resolves the configured compute backend.
    ///
    /// `Backend::Cpu` yields `None`, which keeps the reference path free of any
    /// device setup; `Auto` and `Gpu` yield a device-backed implementation.
    fn compute_backend(&self) -> Result<Option<Box<dyn crate::gpu::ComputeBackend>>> {
        if self.config.backend == Backend::Cpu {
            return Ok(None);
        }
        crate::gpu::select_backend(self.config.backend).map(Some)
    }

    /// Backend used for the batched constraint queries of the lateral offset cap.
    ///
    /// The offset cap can ask its queries through a compute backend, and the answer
    /// is identical either way (a test builds the trajectory both ways and
    /// compares). It is only worth doing when the batch is shared: measured on a
    /// 9 000-sample trajectory, batching the cap costs 22 % more through the CPU
    /// backend and 67 % more through the GPU, because thirteen launches with a
    /// blocking readback each cannot beat in-memory mask lookups at that size. The
    /// design's own framing is the bulk case — many individuals sharing one
    /// read-only field — so the backend is used only when it is asked for
    /// explicitly, and the direct path stays the default.
    fn batch_backend<'a>(
        &self,
        resolved: Option<&'a dyn crate::gpu::ComputeBackend>,
    ) -> Option<&'a dyn crate::gpu::ComputeBackend> {
        match self.config.backend {
            Backend::Gpu => resolved,
            _ => None,
        }
    }

    /// Resolves the cost weights of the configured mode.
    fn weights(&self, map: &Map) -> Result<CostWeights> {
        if let Some(values) = &self.config.weight_override {
            return Ok(CostWeights {
                values: values.clone(),
                scale: self.config.cost.cost_scale,
            });
        }
        let prior = map.weight_prior()?;
        let dimension = map
            .feature_schema()?
            .map(|schema| schema.dim() as usize)
            .unwrap_or(0);
        let Some(entry) = prior.as_ref().and_then(|prior| prior.get(self.config.mode)) else {
            tracing::warn!(
                "map carries no weight prior for mode {:?}; using a uniform weight vector",
                self.config.mode
            );
            return Ok(CostWeights::uniform(dimension));
        };
        match (&self.config.attention, dimension) {
            (Some(gating), _) if !self.config.attention_context.is_empty() => {
                CostWeights::with_attention(
                    entry,
                    gating,
                    &self.config.attention_context,
                    None,
                    Some(self.config.cost.cost_scale),
                )
            }
            (Some(_), _) => {
                // A gate without a context vector cannot be evaluated, and
                // silently running the static table would make the difference
                // invisible to the caller who configured the gate.
                tracing::warn!(
                    "attention gating is configured but the context vector is empty; \
                     using the static weight prior instead"
                );
                Ok(CostWeights::from_prior(
                    entry,
                    None,
                    Some(self.config.cost.cost_scale),
                ))
            }
            _ => Ok(CostWeights::from_prior(
                entry,
                None,
                Some(self.config.cost.cost_scale),
            )),
        }
    }

    /// Runs the simulation.
    pub fn run(&self) -> Result<SimulationOutput> {
        let map = self.source.open()?;
        let weights = self.weights(&map)?;
        let backend = self.compute_backend()?;
        let environment = Environment::load_with_coarse(
            &map,
            &weights,
            &self.config.cost,
            self.config.prm,
            self.config.coarse,
            backend.as_deref(),
        )?;
        let mut graph = MixedGraph::with_coarse(
            &environment.cost,
            &environment.hard,
            Some(&environment.terrain),
            environment.connectors.clone(),
            environment.prm.clone(),
            environment.coarse.clone(),
            self.person.target_speed,
        );

        let route = self.plan_route(&environment, &mut graph)?;
        self.finish_run(&environment, route, backend.as_deref())
    }

    /// Plans the route of the configured mode.
    fn plan_route(
        &self,
        environment: &Environment,
        graph: &mut MixedGraph<'_>,
    ) -> Result<PlannedRoute> {
        match &self.mode {
            PlanMode::Standard(request) => plan(
                environment,
                graph,
                request,
                &self.person,
                &self.config.route,
                self.seed,
                self.individual,
            ),
            PlanMode::Loop(request) => loop_mode::plan_loop(
                environment,
                graph,
                request,
                &self.person,
                &self.config.loop_route,
                self.seed,
                self.individual,
            ),
            PlanMode::Dynamic { request, .. } => plan(
                environment,
                graph,
                request,
                &self.person,
                &self.config.route,
                self.seed,
                self.individual,
            ),
        }
    }

    /// Builds the motion and sensor stages from a planned route.
    ///
    /// `resolved` is the backend the run already resolved, if any. It is recorded
    /// in the manifest as the backend that *ran* rather than the one that was
    /// asked for, which is what makes a provenance record worth keeping: `Auto`
    /// falls back to the CPU without failing, so the configured policy alone
    /// cannot say what produced the data.
    fn finish_run(
        &self,
        environment: &Environment,
        route: PlannedRoute,
        resolved: Option<&dyn crate::gpu::ComputeBackend>,
    ) -> Result<SimulationOutput> {
        let mut motion_config = self.config.motion.clone();
        motion_config.profile.modifiers = route.modifiers.clone();
        motion_config.profile.stops = route.stops.clone();
        // A single lap is a closed circuit and takes the periodic boundary, so the
        // profile closes on itself across the seam. A multi-lap session is not: it
        // starts from rest and ends at rest, and the periodic boundary there leaves
        // the runner at racing speed on the final sample — no deceleration into the
        // finish, no standstill, and the truth differentiation reads that speed
        // straight into the accelerometer.
        motion_config.profile.periodic = self.mode.is_loop() && self.mode.laps() <= 1;
        if self.mode.is_loop() {
            // A lap session runs through the seam: standing still there would
            // break the continuity between laps that the loop mode promises. Only
            // the end of the session gets a standstill.
            motion_config.maneuver.end_stand_s = 0.0;
        }
        // Laps are laid end to end on one path rather than looped at the
        // timeline level: the profile, the noise processes and the bounce phase
        // then continue across the seam with no special case, which is exactly
        // the continuity the design asks for.
        // Reversals get their zero-speed point from the motion stage itself
        // (`Trajectory::build`), which covers every planning mode and keeps the
        // pivot arc equal to the arc the profile brakes to.

        let route = match &self.mode {
            PlanMode::Loop(request) if request.laps > 1 => {
                let mut points = Vec::with_capacity(route.path.points().len() * request.laps);
                for lap in 0..request.laps {
                    let skip = if lap == 0 { 0 } else { 1 };
                    points.extend(route.path.points().iter().skip(skip).copied());
                }
                motion_config.loop_laps = request.laps;
                let mut repeated = route;
                repeated.path = crate::path::Path::new(points)?;
                repeated.length_m = repeated.path.total_length();
                repeated
            }
            _ => route,
        };

        let backend = self.batch_backend(resolved);
        // Connector elevations reach the motion stage through the path and
        // nowhere else: the terrain cannot know about a link. Stamping happens
        // here, on the path every planning mode hands over, and after the lap
        // expansion so a repeated connector is lifted on every lap.
        let path = connector_lift::lift_connector_elevations(
            &route.path,
            &environment.connectors,
            &environment.terrain,
        )?;
        let trajectory = Trajectory::build_with_backend(
            path,
            &environment.terrain,
            &environment.hard,
            &environment.distance,
            &self.person,
            &motion_config,
            self.seed,
            self.individual,
            backend,
        )?;

        let bundle = sensor::generate(
            &trajectory,
            environment.regions.as_ref(),
            environment.magnetic_field.as_ref(),
            environment.frame.as_ref(),
            &self.config.sensors,
            &self.person,
            self.seed,
            self.individual,
        )?;

        let metrics = self.config.with_metrics.then(|| {
            MetricsReport::compute(
                &trajectory,
                Some(&bundle.sensors),
                Some(&bundle.truth),
                self.config.sensors.imu_rate_hz,
                self.config.sensors.baro_rate_hz,
            )
        });

        let (start, goal) = match &self.mode {
            PlanMode::Standard(request) => (request.start, request.goal),
            PlanMode::Loop(request) => (request.start, request.start),
            PlanMode::Dynamic { request, .. } => (request.start, request.goal),
        };

        let manifest = RunManifest {
            generator: format!("ourealis-core {}", env!("CARGO_PKG_VERSION")),
            seed: self.seed,
            individual: self.individual,
            mode: self.mode.name().to_string(),
            map_name: environment
                .map_info
                .as_ref()
                .map(|info| info.name.clone())
                .or_else(|| Some("unnamed".to_string())),
            person: self.person.clone(),
            start: [start.x, start.y],
            goal: [goal.x, goal.y],
            rates_hz: [
                self.config.sensors.gnss_rate_hz,
                self.config.sensors.imu_rate_hz,
                self.config.sensors.mag_rate_hz,
                self.config.sensors.baro_rate_hz,
            ],
            backend: resolved
                .map(|backend| backend.name())
                .unwrap_or_else(|| output::backend_name(self.config.backend).to_string()),
            mount: self.config.sensors.mount.name().to_string(),
            duration_s: trajectory.duration_s(),
        };

        Ok(SimulationOutput {
            manifest,
            truth: bundle.truth,
            sensors: bundle.sensors,
            route: RouteSummary::from_route(&route),
            metrics,
            trajectory,
        })
    }

    /// Runs the simulation with online re-planning for its checkpoints.
    pub fn run_dynamic(&self) -> Result<SimulationOutput> {
        let PlanMode::Dynamic {
            request,
            checkpoints,
        } = &self.mode
        else {
            return self.run();
        };
        let map = self.source.open()?;
        let weights = self.weights(&map)?;
        let backend = self.compute_backend()?;
        let environment = Environment::load_with_coarse(
            &map,
            &weights,
            &self.config.cost,
            self.config.prm,
            self.config.coarse,
            backend.as_deref(),
        )?;
        let mut graph = MixedGraph::with_coarse(
            &environment.cost,
            &environment.hard,
            Some(&environment.terrain),
            environment.connectors.clone(),
            environment.prm.clone(),
            environment.coarse.clone(),
            self.person.target_speed,
        );

        let mut route = plan(
            &environment,
            &mut graph,
            request,
            &self.person,
            &self.config.route,
            self.seed,
            self.individual,
        )?;

        // Build the initial trajectory, then fold in every checkpoint in time
        // order, blending each redirect into the running trajectory.
        let mut output = self.finish_run(&environment, route.clone(), backend.as_deref())?;
        for checkpoint in checkpoints {
            let (new_route, record) = dynamic::replan(
                &environment,
                &mut graph,
                &output.trajectory,
                checkpoint,
                &self.person,
                &self.config.dynamic,
                self.seed,
                self.individual,
            )?;
            if !record.accepted {
                tracing::debug!("checkpoint ignored: {}", record.note);
                continue;
            }
            let mut motion_config = self.config.motion.clone();
            motion_config.profile.modifiers = new_route.modifiers.clone();
            motion_config.profile.stops = new_route.stops.clone();
            // The runner is already moving: a redirect has no standing start, and
            // the blend window supplies the transition instead. The profile keeps
            // the speed the runner carries at the switch, otherwise the rebuilt
            // leg would brake to a halt and accelerate again.
            motion_config.maneuver.start_stand_s = 0.0;
            motion_config.maneuver.end_stand_s = 0.0;
            motion_config.profile.initial_speed = output
                .trajectory
                .sample_at(
                    checkpoint
                        .issued_at_s
                        .clamp(0.0, output.trajectory.duration_s()),
                )
                .map(|sample| sample.speed);
            // Reversals on the new leg register their own zero-speed points in the
            // motion stage, so nothing is added here.
            // The replacement runs with the individual's own index: the step phase
            // is per-individual and shared by the bounce, the accelerometer
            // harmonic and the gyroscope, so perturbing it here would put the
            // accelerometer of the whole recording out of phase with the height
            // that the barometer sees.
            let replacement = Trajectory::build_with_backend(
                connector_lift::lift_connector_elevations(
                    &new_route.path,
                    &environment.connectors,
                    &environment.terrain,
                )?,
                &environment.terrain,
                &environment.hard,
                &environment.distance,
                &self.person,
                &motion_config,
                self.seed,
                self.individual,
                self.batch_backend(backend.as_deref()),
            )?;
            let blended = dynamic::blend_trajectories(
                &output.trajectory,
                &replacement,
                checkpoint.issued_at_s,
                self.config.dynamic.blend_window_s,
                &environment.hard,
            )?;
            let bundle = sensor::generate(
                &blended,
                environment.regions.as_ref(),
                environment.magnetic_field.as_ref(),
                environment.frame.as_ref(),
                &self.config.sensors,
                &self.person,
                self.seed,
                self.individual,
            )?;
            let metrics = self.config.with_metrics.then(|| {
                MetricsReport::compute(
                    &blended,
                    Some(&bundle.sensors),
                    Some(&bundle.truth),
                    self.config.sensors.imu_rate_hz,
                    self.config.sensors.baro_rate_hz,
                )
            });
            route = new_route;
            output.trajectory = blended;
            output.truth = bundle.truth;
            output.sensors = bundle.sensors;
            output.metrics = metrics;
            output.route = RouteSummary::from_route(&route);
        }
        output.manifest.duration_s = output.trajectory.duration_s();
        Ok(output)
    }
}

/// Runs a population of individuals over the same route configuration.
#[derive(Debug, Clone)]
pub struct BatchRunner {
    simulator: Simulator,
}

impl BatchRunner {
    /// Creates a batch runner from a configured simulator.
    pub fn new(simulator: Simulator) -> Self {
        Self { simulator }
    }

    /// Runs one individual per entry of `people`.
    ///
    /// Individuals are independent and are distributed over the rayon pool. The
    /// random streams are keyed by the individual index rather than by thread, so
    /// a parallel batch produces exactly the same output as a serial one.
    pub fn run(&self, people: &[PersonParams]) -> Result<Vec<SimulationOutput>> {
        let map = self.simulator.source.open()?;
        let weights = self.simulator.weights(&map)?;
        let backend = self.simulator.compute_backend()?;
        let environment = Environment::load_with_coarse(
            &map,
            &weights,
            &self.simulator.config.cost,
            self.simulator.config.prm,
            self.simulator.config.coarse,
            backend.as_deref(),
        )?;

        let results: Result<Vec<SimulationOutput>> = people
            .par_iter()
            .enumerate()
            .map(|(index, person)| {
                person.validate()?;
                let mut graph = MixedGraph::with_coarse(
                    &environment.cost,
                    &environment.hard,
                    Some(&environment.terrain),
                    environment.connectors.clone(),
                    environment.prm.clone(),
                    environment.coarse.clone(),
                    person.target_speed,
                );
                let route = match &self.simulator.mode {
                    PlanMode::Standard(request) => plan(
                        &environment,
                        &mut graph,
                        request,
                        person,
                        &self.simulator.config.route,
                        self.simulator.seed,
                        index as u32,
                    )?,
                    PlanMode::Loop(request) => loop_mode::plan_loop(
                        &environment,
                        &mut graph,
                        request,
                        person,
                        &self.simulator.config.loop_route,
                        self.simulator.seed,
                        index as u32,
                    )?,
                    PlanMode::Dynamic { request, .. } => plan(
                        &environment,
                        &mut graph,
                        request,
                        person,
                        &self.simulator.config.route,
                        self.simulator.seed,
                        index as u32,
                    )?,
                };
                let mut per_person = self.simulator.clone();
                per_person.person = person.clone();
                per_person.individual = index as u32;
                per_person.finish_run(&environment, route, backend.as_deref())
            })
            .collect();
        results
    }

    /// Regresses mean speed against step frequency across a population.
    ///
    /// The design's parameter table gives cadence a weak positive correlation
    /// with speed, and the population sampler is built to reproduce it. This
    /// measures whether the *realised* runs do, which is a different question:
    /// a bug in the sampling or in the speed limits would break the relation
    /// without touching the parameters.
    pub fn cadence_speed_fit(
        &self,
        outputs: &[SimulationOutput],
    ) -> Option<crate::eval::LinearFit> {
        let cadences: Vec<f64> = outputs
            .iter()
            .map(|output| output.manifest.person.step_frequency)
            .collect();
        let speeds: Vec<f64> = outputs
            .iter()
            .map(|output| {
                output
                    .metrics
                    .as_ref()
                    .map(|metrics| metrics.speed.mean)
                    .unwrap_or(0.0)
            })
            .collect();
        crate::eval::LinearFit::of(&cadences, &speeds)
    }

    /// KS statistic between a reference speed distribution and a batch.
    pub fn speed_ks_against(&self, reference: &[f64], outputs: &[SimulationOutput]) -> f64 {
        let simulated = crate::eval::pooled_speed_samples(outputs);
        crate::eval::ks_statistic(reference, &simulated)
    }

    /// Runs a population and reports the path-choice frequencies.
    ///
    /// Reproduces the design's path-frequency metric: with one implementation of
    /// candidate generation for stored and on-line paths, these counts are
    /// comparable across runs.
    pub fn choice_frequencies(&self, outputs: &[SimulationOutput]) -> Vec<f64> {
        let choices: Vec<usize> = outputs
            .iter()
            .filter_map(|output| output.route.chosen.first().copied())
            .collect();
        let candidate_count = outputs
            .first()
            .and_then(|output| output.route.legs.first())
            .map(|legs| legs.len())
            .unwrap_or(0);
        crate::eval::choice_frequencies(&choices, candidate_count)
    }
}

/// Default smoothing configuration, exposed for callers that build routes by hand.
pub fn default_smoothing() -> ElasticBandConfig {
    ElasticBandConfig::default()
}

/// Convenience: a synthetic map source sized for examples.
pub fn example_map_source() -> MapSource {
    MapSource::synthetic(SyntheticMapSpec::default())
}
