//! Job execution: capacity gate, worker dispatch and the run itself.
//!
//! The simulator is synchronous and CPU-bound, so a job runs on a blocking worker.
//! Capacity is a semaphore of `simulation.max_concurrent` permits; a submission
//! that arrives while every permit is taken waits, and one that arrives while the
//! queue is also full is rejected with `busy` rather than piling up.

use std::sync::Arc;

use ourealis_core::plan::Checkpoint;
use ourealis_core::sim::{MapSource, Simulator};
use tokio::sync::Semaphore;

use crate::api::dto::SimulationRequest;
use crate::error::{Result, ServiceError};
use crate::store::MapStore;

use super::{Job, JobRegistry};

/// Everything a job needs that outlives it.
#[derive(Debug)]
pub struct JobContext {
    /// Map library.
    pub maps: Arc<dyn MapStore>,
    /// Capacity and queueing.
    pub max_concurrent: usize,
    /// Jobs allowed to wait beyond the running ones.
    pub queue_capacity: usize,
    /// Whether runs evaluate their metrics report by default.
    pub with_metrics: bool,
}

/// Submits and runs jobs.
#[derive(Debug)]
pub struct JobRunner {
    context: Arc<JobContext>,
    registry: Arc<JobRegistry>,
    permits: Arc<Semaphore>,
}

impl JobRunner {
    /// Creates a runner and its capacity gate.
    pub fn new(context: Arc<JobContext>, registry: Arc<JobRegistry>) -> Self {
        let permits = Arc::new(Semaphore::new(context.max_concurrent.max(1)));
        Self {
            context,
            registry,
            permits,
        }
    }

    /// Jobs that are queued or running.
    pub fn in_flight(&self) -> usize {
        self.registry.active()
    }

    /// Admits a job and starts it.
    ///
    /// The map is resolved here rather than on the worker: a submission that names
    /// a map that does not exist must fail immediately, while the client can still
    /// associate the error with its request.
    pub async fn submit(&self, id: String, request: SimulationRequest) -> Result<Arc<Job>> {
        let map_id = self.resolve_map(&request)?;
        let capacity = self.context.max_concurrent + self.context.queue_capacity;
        let job = Job::new(id, request, map_id);
        self.registry.try_admit(Arc::clone(&job), capacity)?;
        job.log("info", format!("job queued for map {}", job.map_id()));

        let runner = Self {
            context: Arc::clone(&self.context),
            registry: Arc::clone(&self.registry),
            permits: Arc::clone(&self.permits),
        };
        let worker_job = Arc::clone(&job);
        tokio::spawn(async move { runner.execute(worker_job).await });
        Ok(job)
    }

    /// Runs one job: take a permit, run the simulator, record the outcome.
    async fn execute(self, job: Arc<Job>) {
        let Ok(permit) = self.permits.clone().acquire_owned().await else {
            job.mark_failed("internal", "the job gate was closed");
            return;
        };
        if job.is_cancelled() {
            drop(permit);
            job.mark_cancelled();
            return;
        }
        job.mark_running();

        // One handle for the worker and one for the bookkeeping that follows it.
        let worker_context = Arc::clone(&self.context);
        let context = Arc::clone(&self.context);
        let worker_job = Arc::clone(&job);
        let outcome =
            tokio::task::spawn_blocking(move || run_simulation(&worker_context, &worker_job)).await;
        match outcome {
            Ok(Ok(output)) => {
                let output = Arc::new(output);
                job.log(
                    "info",
                    format!(
                        "run finished: {:.1} m over {:.1} s, {} truth sample(s)",
                        output.route.length_m,
                        output.duration_s(),
                        output.truth_len()
                    ),
                );
                // Persisting the summary is best effort: a store that cannot write is
                // a logging concern, not a reason to fail a run whose data is in memory
                // and reproducible from its seed.
                match serde_json::from_str::<serde_json::Value>(&output.summary_json()) {
                    Ok(summary) => {
                        if let Err(error) = context.maps.save_run_summary(job.id(), &summary) {
                            job.log(
                                "warn",
                                format!("the run summary was not persisted: {error}"),
                            );
                        }
                    }
                    Err(error) => {
                        job.log("warn", format!("the run summary is not readable: {error}"));
                    }
                }
                job.mark_succeeded(output);
            }
            Ok(Err(error)) => {
                let kind = error.kind().as_str().to_string();
                job.log("error", error.to_string());
                job.mark_failed(&kind, &error.to_string());
            }
            Err(join_error) => {
                let message = if join_error.is_panic() {
                    "the simulation worker panicked; the request may have hit a defect".to_string()
                } else {
                    format!("the simulation worker was cancelled: {join_error}")
                };
                job.log("error", message.clone());
                job.mark_failed("internal", &message);
            }
        }
        drop(permit);
    }

    /// Resolves the map a request names, defaulting to the only map in the library.
    fn resolve_map(&self, request: &SimulationRequest) -> Result<String> {
        use crate::api::dto::simulation::MapRef;
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

    /// Registry of this runner's jobs.
    pub fn registry(&self) -> &Arc<JobRegistry> {
        &self.registry
    }
}

/// Runs one simulation on a blocking worker.
///
/// The map image is materialised here, which is why this is not async: opening a
/// map and building the environment is the expensive part of a run and it belongs
/// on the same worker as the run itself.
fn run_simulation(context: &JobContext, job: &Job) -> Result<ourealis_core::SimulationOutput> {
    // The request is borrowed through the job's lock for the whole run, so a terminal
    // transition on another thread cannot free the payload under the worker.
    job.with_request(|request| run_with_request(context, job, request))?
}

/// Runs the simulation for one request.
fn run_with_request(
    context: &JobContext,
    job: &Job,
    request: &SimulationRequest,
) -> Result<ourealis_core::SimulationOutput> {
    use crate::api::dto::simulation::MapRef;

    let source = match &request.map {
        Some(MapRef::Inline {
            omf_base64,
            name: _,
        }) => {
            let bytes = crate::api::base64::decode(omf_base64, "inline map")?;
            MapSource::bytes(bytes)
        }
        Some(MapRef::Synthetic { spec }) => MapSource::synthetic(spec.to_spec()),
        Some(MapRef::Id { id }) => {
            let entry = context.maps.get(id)?;
            MapSource::bytes(entry.bytes.as_ref().clone())
        }
        None => {
            let entry = context.maps.get(job.map_id())?;
            MapSource::bytes(entry.bytes.as_ref().clone())
        }
    };

    let person = request.person.resolve()?;
    let mut settings = ourealis_core::sim::SimulationConfig::default();
    request.settings.apply(&mut settings)?;
    if request.settings.with_metrics.is_none() {
        settings.with_metrics = context.with_metrics;
    }
    settings.sensors.validate()?;

    let simulator = match &request.route {
        crate::api::dto::simulation::RouteSpec::Standard { .. } => Simulator::builder()
            .map(source)
            .person(person)
            .standard(request.standard_request()?)
            .config(settings)
            .seed(request.seed)
            .individual(request.individual)
            .build()?,
        crate::api::dto::simulation::RouteSpec::Loop { .. } => Simulator::builder()
            .map(source)
            .person(person)
            .looped(request.loop_request()?)
            .config(settings)
            .seed(request.seed)
            .individual(request.individual)
            .build()?,
        crate::api::dto::simulation::RouteSpec::Dynamic { .. } => {
            let (standard, checkpoints) = request.dynamic_request()?;
            Simulator::builder()
                .map(source)
                .person(person)
                .dynamic(standard, checkpoints)
                .config(settings)
                .seed(request.seed)
                .individual(request.individual)
                .build()?
        }
    };

    job.log(
        "info",
        format!(
            "running {} route with seed {} on map {}",
            request.mode_name(),
            request.seed,
            job.map_id()
        ),
    );
    let output = match &request.route {
        // A dynamic run only differs once a checkpoint takes effect, but the
        // timeline rewrite lives behind `run_dynamic`; calling it with an empty
        // checkpoint list would be the same run with extra bookkeeping, so the
        // plain entry point is used there.
        crate::api::dto::simulation::RouteSpec::Dynamic { checkpoints, .. }
            if !checkpoints.is_empty() =>
        {
            simulator.run_dynamic()?
        }
        _ => simulator.run()?,
    };
    Ok(output)
}

/// Checkpoints of a request, exposed for the dynamic endpoints.
pub fn checkpoints_of(request: &SimulationRequest) -> Result<Vec<Checkpoint>> {
    let (_, checkpoints) = request.dynamic_request()?;
    Ok(checkpoints)
}
