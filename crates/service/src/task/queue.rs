//! Task execution: capacity gate, worker dispatch and the task bodies.
//!
//! Every task body is synchronous and CPU-bound, so it runs on a blocking worker.
//! Capacity is a semaphore of `simulation.max_concurrent` permits shared by every
//! kind: a run and a route plan compete for the same CPU, so they must not be
//! admitted separately. A submission that arrives while every permit is taken waits,
//! and one that arrives while the queue is also full is rejected with `busy` rather
//! than piling up.

use std::sync::Arc;
use std::time::Instant;

use ourealis_core::environment::Environment;
use ourealis_core::graph::MixedGraph;
use ourealis_core::motion::profile::SpeedProfile;
use ourealis_core::person::PersonParams;
use ourealis_core::plan::{PlannedRoute, plan, plan_loop};
use ourealis_core::sim::{MapSource, SimulationConfig, Simulator};
use ourealis_map_format::synthetic::SyntheticMapSpec;
use tokio::sync::Semaphore;

use crate::api::dto::simulation::MapRef;
use crate::api::dto::{MapSummary, RoutePreview, RoutePreviewCandidate, SimulationRequest};
use crate::error::{Result, ServiceError};
use crate::store::MapStore;

use super::{Task, TaskOutcome, TaskPayload, TaskRegistry};

/// Everything a task needs that outlives it.
#[derive(Debug)]
pub struct TaskContext {
    /// Map library.
    pub maps: Arc<dyn MapStore>,
    /// Capacity and queueing.
    pub max_concurrent: usize,
    /// Tasks allowed to wait beyond the running ones.
    pub queue_capacity: usize,
    /// Whether runs evaluate their metrics report by default.
    pub with_metrics: bool,
}

/// Submits and runs tasks.
#[derive(Debug)]
pub struct TaskRunner {
    context: Arc<TaskContext>,
    registry: Arc<TaskRegistry>,
    permits: Arc<Semaphore>,
}

impl TaskRunner {
    /// Creates a runner and its capacity gate.
    pub fn new(context: Arc<TaskContext>, registry: Arc<TaskRegistry>) -> Self {
        let permits = Arc::new(Semaphore::new(context.max_concurrent.max(1)));
        Self {
            context,
            registry,
            permits,
        }
    }

    /// Tasks that are queued or running.
    pub fn in_flight(&self) -> usize {
        self.registry.active()
    }

    /// Registry of this runner's tasks.
    pub fn registry(&self) -> &Arc<TaskRegistry> {
        &self.registry
    }

    /// Admits a run and starts it.
    ///
    /// The map is resolved here rather than on the worker: a submission that names a
    /// map that does not exist must fail immediately, while the client can still
    /// associate the error with its request.
    pub async fn submit_simulation(
        &self,
        id: String,
        request: SimulationRequest,
    ) -> Result<Arc<Task>> {
        let map_id = self.resolve_map(&request)?;
        let mode = request.mode_name().to_string();
        self.admit(id, TaskPayload::Simulation(request), map_id, mode, "run")
    }

    /// Admits a planning task and starts it.
    ///
    /// `with_profile` asks for the smoothed path and its speed limits as well as the
    /// candidate set, which is the only difference the caller sees between a preview
    /// and a plan.
    pub async fn submit_planning(
        &self,
        id: String,
        request: SimulationRequest,
        with_profile: bool,
    ) -> Result<Arc<Task>> {
        let map_id = self.resolve_map(&request)?;
        let mode = request.mode_name().to_string();
        let label = if with_profile { "plan" } else { "preview" };
        self.admit(
            id,
            TaskPayload::Planning {
                request,
                with_profile,
            },
            map_id,
            mode,
            label,
        )
    }

    /// Admits a synthetic map build and starts it.
    ///
    /// The generator is bounded by `SyntheticMapSpec::validate`, but a large map still
    /// takes tens of seconds to rasterise and encode, so it belongs on a worker behind
    /// a ticket rather than on the request path.
    pub async fn submit_synthetic_map(
        &self,
        id: String,
        spec: SyntheticMapSpec,
        name: Option<String>,
    ) -> Result<Arc<Task>> {
        let resolution = spec.effective_resolution_m();
        self.admit(
            id,
            TaskPayload::SyntheticMap { spec, name },
            "synthetic".to_string(),
            format!("synthetic@{resolution}"),
            "map",
        )
    }

    /// Admits a payload and spawns its worker.
    fn admit(
        &self,
        id: String,
        payload: TaskPayload,
        map_id: String,
        mode: String,
        what: &str,
    ) -> Result<Arc<Task>> {
        let capacity = self.context.max_concurrent + self.context.queue_capacity;
        let task = Task::new(id, payload, map_id.clone(), mode);
        self.registry.try_admit(Arc::clone(&task), capacity)?;
        task.log("info", format!("{what} queued for map {map_id}"));

        let runner = Self {
            context: Arc::clone(&self.context),
            registry: Arc::clone(&self.registry),
            permits: Arc::clone(&self.permits),
        };
        let worker = Arc::clone(&task);
        tokio::spawn(async move { runner.execute(worker).await });
        Ok(task)
    }

    /// Runs one task: take a permit, run its body, record the outcome.
    async fn execute(self, task: Arc<Task>) {
        let Ok(permit) = self.permits.clone().acquire_owned().await else {
            task.mark_failed("internal", "the task gate was closed");
            return;
        };
        if task.is_cancelled() {
            drop(permit);
            task.mark_cancelled();
            return;
        }
        task.mark_running();

        // One handle for the worker and one for the bookkeeping that follows it.
        let worker_context = Arc::clone(&self.context);
        let context = Arc::clone(&self.context);
        let worker_task = Arc::clone(&task);
        let outcome =
            tokio::task::spawn_blocking(move || run_task(&worker_context, &worker_task)).await;
        match outcome {
            Ok(Ok(result)) => {
                if let Some(output) = result.simulation() {
                    task.log(
                        "info",
                        format!(
                            "run finished: {:.1} m over {:.1} s, {} truth sample(s)",
                            output.route.length_m,
                            output.duration_s(),
                            output.truth_len()
                        ),
                    );
                    // Persisting the summary is best effort: a store that cannot write
                    // is a logging concern, not a reason to fail a run whose data is in
                    // memory and reproducible from its seed.
                    match serde_json::from_str::<serde_json::Value>(&output.summary_json()) {
                        Ok(summary) => {
                            if let Err(error) = context.maps.save_run_summary(task.id(), &summary) {
                                task.log(
                                    "warn",
                                    format!("the run summary was not persisted: {error}"),
                                );
                            }
                        }
                        Err(error) => {
                            task.log("warn", format!("the run summary is not readable: {error}"));
                        }
                    }
                }
                task.mark_succeeded(result);
            }
            Ok(Err(error)) => {
                let kind = error.kind().as_str().to_string();
                task.log("error", error.to_string());
                task.mark_failed(&kind, &error.to_string());
            }
            Err(join_error) => {
                let message = if join_error.is_panic() {
                    "the worker panicked; the request may have hit a defect".to_string()
                } else {
                    format!("the worker was cancelled: {join_error}")
                };
                task.log("error", message.clone());
                task.mark_failed("internal", &message);
            }
        }
        drop(permit);
    }

    /// Resolves the map a request names, defaulting to the only map in the library.
    fn resolve_map(&self, request: &SimulationRequest) -> Result<String> {
        match &request.map {
            Some(MapRef::Id { id }) => {
                self.context.maps.get(id)?;
                Ok(id.clone())
            }
            Some(MapRef::Synthetic { .. }) | Some(MapRef::Inline { .. }) => {
                Ok("inline".to_string())
            }
            None => {
                let maps = self.context.maps.list();
                match maps.len() {
                    0 => Err(ServiceError::Invalid(
                        "no map was given and the library is empty; add a map or name one"
                            .to_string(),
                    )),
                    1 => Ok(maps[0].id.clone()),
                    _ => Err(ServiceError::Invalid(format!(
                        "no map was given and the library holds {} maps; name one",
                        maps.len()
                    ))),
                }
            }
        }
    }
}

/// Runs one task body on a blocking worker.
fn run_task(context: &TaskContext, task: &Task) -> Result<TaskOutcome> {
    // The payload is borrowed through the task's lock for the whole run, so a terminal
    // transition on another thread cannot free it under the worker.
    task.with_payload(|payload| match payload {
        TaskPayload::Simulation(request) => {
            run_simulation(context, task, request).map(TaskOutcome::Simulation)
        }
        TaskPayload::Planning {
            request,
            with_profile,
        } => run_planning(context, task, request, *with_profile)
            .map(|preview| TaskOutcome::Route(Box::new(preview))),
        TaskPayload::SyntheticMap { spec, name } => {
            run_synthetic_map(context, task, spec, name.as_deref())
                .map(|summary| TaskOutcome::Map(Box::new(summary)))
        }
    })?
}

/// Runs one simulation.
///
/// The map image is materialised here, which is why this is not async: opening a map
/// and building the environment is the expensive part of a run and it belongs on the
/// same worker as the run itself.
fn run_simulation(
    context: &TaskContext,
    task: &Task,
    request: &SimulationRequest,
) -> Result<Arc<ourealis_core::SimulationOutput>> {
    let source = map_source(context, task, request)?;
    let person = request.person.resolve()?;
    let mut settings = SimulationConfig::default();
    request.settings.apply(&mut settings)?;
    if request.settings.with_metrics.is_none() {
        settings.with_metrics = context.with_metrics;
    }
    settings.sensors.validate()?;

    let simulator = build_simulator(source, request, person, settings)?;
    task.log(
        "info",
        format!(
            "running {} route with seed {} on map {}",
            request.mode_name(),
            request.seed,
            task.map_id()
        ),
    );
    let output = match &request.route {
        // A dynamic run only differs once a checkpoint takes effect, but the timeline
        // rewrite lives behind `run_dynamic`; calling it with an empty checkpoint list
        // would be the same run with extra bookkeeping, so the plain entry point is
        // used there.
        crate::api::dto::simulation::RouteSpec::Dynamic { checkpoints, .. }
            if !checkpoints.is_empty() =>
        {
            simulator.run_dynamic()?
        }
        _ => simulator.run()?,
    };
    Ok(Arc::new(output))
}

/// Builds the simulator a request describes, whatever its planning mode.
fn build_simulator(
    source: MapSource,
    request: &SimulationRequest,
    person: PersonParams,
    settings: SimulationConfig,
) -> Result<Simulator> {
    let builder = Simulator::builder()
        .map(source)
        .person(person)
        .config(settings)
        .seed(request.seed)
        .individual(request.individual);
    Ok(match &request.route {
        crate::api::dto::simulation::RouteSpec::Standard { .. } => {
            builder.standard(request.standard_request()?).build()?
        }
        crate::api::dto::simulation::RouteSpec::Loop { .. } => {
            builder.looped(request.loop_request()?).build()?
        }
        crate::api::dto::simulation::RouteSpec::Dynamic { .. } => {
            let (standard, checkpoints) = request.dynamic_request()?;
            builder.dynamic(standard, checkpoints).build()?
        }
    })
}

/// Plans a route and reports the candidate set, and its profile when asked for.
///
/// This is the whole body of a planning task: the planner runs exactly as a run does —
/// same environment, same mixed graph, same configuration and seed — and stops before
/// the motion stage. That is what makes a preview a prediction of the run rather than
/// a second, independent planner.
pub(crate) fn run_planning(
    context: &TaskContext,
    task: &Task,
    request: &SimulationRequest,
    with_profile: bool,
) -> Result<RoutePreview> {
    let source = map_source(context, task, request)?;
    let person = request.person.resolve()?;
    let mut config = SimulationConfig::default();
    request.settings.apply(&mut config)?;

    // The environment is loaded by the simulator itself, which is what keeps the
    // weights, the backend selection and the coarse grid identical to a run.
    let simulator = build_simulator(source, request, person.clone(), config.clone())?;
    let environment = simulator.environment()?;
    let mut graph = MixedGraph::with_coarse(
        &environment.cost,
        &environment.hard,
        Some(&environment.terrain),
        environment.connectors.clone(),
        environment.prm.clone(),
        environment.coarse.clone(),
        person.target_speed,
    );

    task.log(
        "info",
        format!(
            "planning {} route with seed {} on map {}",
            request.mode_name(),
            request.seed,
            task.map_id()
        ),
    );
    let started = Instant::now();
    let planned = plan_route_of(&environment, &mut graph, request, &person, &config)?;
    let planning_ms = started.elapsed().as_secs_f64() * 1000.0;

    let from_library = request.route.mode_name() != "loop"
        && library_supplies(
            &environment,
            &mut graph,
            &person,
            &config,
            planned.legs.first().map(|leg| leg.from),
            planned.legs.last().map(|leg| leg.to),
        );

    let mut preview = RoutePreview {
        candidates: candidates_of(&planned, from_library),
        chosen: planned
            .legs
            .first()
            .map(|leg| leg.chosen)
            .unwrap_or_default(),
        length_m: planned.length_m,
        cost_equiv_m: planned.cost_equiv_m,
        straight_line_m: straight_line_m(&planned),
        planning_ms,
        path: Vec::new(),
        speed_limit_mps: Vec::new(),
        speed_limit_s: Vec::new(),
    };

    if with_profile {
        let profile = speed_profile(&environment, &person, &config, request, &planned)?;
        preview.speed_limit_s = profile.s.clone();
        preview.speed_limit_mps = profile.s.iter().map(|arc| profile.limit_at(*arc)).collect();
        preview.path = planned
            .path
            .points()
            .iter()
            .map(|point| crate::api::dto::Vec2::from(*point))
            .collect();
    }
    task.log(
        "info",
        format!(
            "planned {:.1} m in {planning_ms:.0} ms with {} candidate(s)",
            preview.length_m,
            preview.candidates.len()
        ),
    );
    Ok(preview)
}

/// Generates a synthetic map and files it in the library.
fn run_synthetic_map(
    context: &TaskContext,
    task: &Task,
    spec: &SyntheticMapSpec,
    name: Option<&str>,
) -> Result<MapSummary> {
    let (width, height) = spec.cell_dims();
    task.log(
        "info",
        format!(
            "generating a {width} x {height} cell map at a seed of {}",
            spec.seed
        ),
    );
    let started = Instant::now();
    let bytes = ourealis_map_format::synthetic::build(spec)?;
    task.log(
        "info",
        format!(
            "generated {} byte(s) in {:.0} ms",
            bytes.len(),
            started.elapsed().as_secs_f64() * 1000.0
        ),
    );
    crate::api::maps::add_image(context.maps.as_ref(), &bytes, "synthetic", name)
}

/// Plans the route of the request's mode.
fn plan_route_of(
    environment: &Environment,
    graph: &mut MixedGraph<'_>,
    request: &SimulationRequest,
    person: &PersonParams,
    config: &SimulationConfig,
) -> Result<PlannedRoute> {
    Ok(match &request.route {
        crate::api::dto::simulation::RouteSpec::Loop { .. } => plan_loop(
            environment,
            graph,
            &request.loop_request()?,
            person,
            &config.loop_route,
            request.seed,
            request.individual,
        )?,
        _ => plan(
            environment,
            graph,
            &request.standard_request()?,
            person,
            &config.route,
            request.seed,
            request.individual,
        )?,
    })
}

/// The candidate set of the route.
///
/// A multi-leg request is planned leg by leg with its own Logit draw, and the preview
/// carries one set, so the first leg's candidates are the ones reported; `chosen`
/// indexes that same set.
fn candidates_of(planned: &PlannedRoute, from_library: bool) -> Vec<RoutePreviewCandidate> {
    let Some(leg) = planned.legs.first() else {
        return Vec::new();
    };
    let probabilities = leg.candidates.probabilities();
    leg.candidates
        .candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| RoutePreviewCandidate {
            length_m: candidate.length_m,
            cost_equiv_m: candidate.cost_equiv_m,
            probability: probabilities.get(index).copied().unwrap_or(0.0),
            path_size: candidate.path_size,
            from_library,
            points: candidate
                .points
                .iter()
                .map(|point| crate::api::dto::Vec2::from(*point))
                .collect(),
        })
        .collect()
}

/// Straight-line distance between the ends of the planned route.
fn straight_line_m(planned: &PlannedRoute) -> f64 {
    match (planned.legs.first(), planned.legs.last()) {
        (Some(first), Some(last)) => first.from.distance(last.to),
        _ => 0.0,
    }
}

/// Whether the map's stored candidate library supplies the planned leg.
///
/// The planner does not report where a candidate came from, so the question is
/// answered by repeating the library lookup it makes: same endpoints, same parameters,
/// and the same graph — whose caches the run has already warmed — so the answer is the
/// one the run acted on.
fn library_supplies(
    environment: &Environment,
    graph: &mut MixedGraph<'_>,
    person: &PersonParams,
    config: &SimulationConfig,
    from: Option<glam::DVec2>,
    to: Option<glam::DVec2>,
) -> bool {
    let (Some(from), Some(to)) = (from, to) else {
        return false;
    };
    let hit = ourealis_core::plan::library::from_library(
        environment.kpath.as_ref(),
        graph,
        from,
        to,
        person.beta_logit,
        &config.route.candidates,
        &config.route.search,
        config.route.d_attach_m,
    );
    matches!(hit, Ok(Ok(_)))
}

/// Speed-limit curve of the planned route, sampled on the profile grid.
fn speed_profile(
    environment: &Environment,
    person: &PersonParams,
    config: &SimulationConfig,
    request: &SimulationRequest,
    planned: &PlannedRoute,
) -> Result<SpeedProfile> {
    let mut profile = config.motion.profile.clone();
    profile.modifiers = planned.modifiers.clone();
    profile.stops = planned.stops.clone();
    // A single-lap circuit closes on itself across the seam; everything else starts
    // from rest and ends at rest, exactly as the motion stage builds it.
    let laps = match &request.route {
        crate::api::dto::simulation::RouteSpec::Loop { laps, .. } => *laps,
        _ => 0,
    };
    profile.periodic = request.route.mode_name() == "loop" && laps <= 1;
    Ok(SpeedProfile::build(
        &planned.path,
        &environment.terrain,
        &config.motion.limits,
        person,
        &profile,
        &[],
    )?)
}

/// Resolves the map a request names.
///
/// An id must exist, an inline image is decoded, a synthetic spec is built, and a
/// request without a map needs a library holding exactly one.
fn map_source(
    context: &TaskContext,
    task: &Task,
    request: &SimulationRequest,
) -> Result<MapSource> {
    match &request.map {
        Some(MapRef::Id { id }) => {
            let entry = context.maps.get(id)?;
            Ok(MapSource::bytes(entry.bytes.as_ref().clone()))
        }
        Some(MapRef::Synthetic { spec }) => Ok(MapSource::synthetic(spec.to_spec()?)),
        Some(MapRef::Inline { omf_base64, .. }) => Ok(MapSource::bytes(
            crate::api::maps::decode_base64(omf_base64)?,
        )),
        None => {
            let entry = context.maps.get(task.map_id())?;
            Ok(MapSource::bytes(entry.bytes.as_ref().clone()))
        }
    }
}
