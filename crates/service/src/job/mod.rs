//! Simulation jobs: state machine, event stream and execution.
//!
//! A submission returns an identifier immediately; the work happens on a blocking
//! worker so the async reactor is never held up by a run that takes minutes. What
//! a client can observe is the state, the stage, the elapsed time and a stream of
//! events — the simulator's own run is a single call, so the service reports
//! *what it started* rather than inventing a percentage.

pub mod events;
pub mod queue;

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use ourealis_core::SimulationOutput;
use tokio::sync::broadcast;

use crate::api::dto::{JobStateDto, SimulationRequest, SimulationStateDto};
use crate::api::time::{now_unix_ms, unix_ms_to_rfc3339};
use crate::error::{Result, ServiceError};

pub use events::EventBus;
pub use queue::{JobContext, JobRunner};

/// Stage names the service reports.
pub mod stage {
    /// Waiting for a worker slot.
    pub const QUEUED: &str = "queued";
    /// A worker holds the job and the simulator is running.
    pub const RUNNING: &str = "running";
    /// The job finished and its result is available.
    pub const DONE: &str = "done";
    /// The job failed.
    pub const FAILED: &str = "failed";
    /// The job was cancelled.
    pub const CANCELLED: &str = "cancelled";
}

/// Number of events a slow subscriber may fall behind before it is notified of
/// the gap rather than blocking the producer.
const EVENT_BACKLOG: usize = 1024;

/// One simulation job.
#[derive(Debug)]
pub struct Job {
    id: String,
    name: Option<String>,
    map_id: String,
    mode: String,
    /// The request as submitted. Locked rather than plain so a finished job can shed
    /// an inline image: a 256 MiB upload must not stay resident for the life of the
    /// registry just because the job that carried it is done.
    request: RwLock<SimulationRequest>,
    created_at_ms: i64,
    started_at_ms: AtomicI64,
    finished_at_ms: AtomicI64,
    /// [`JobStateDto`] as a byte; atomics keep the state readable without a lock
    /// from the event stream and the handlers at once.
    state: AtomicU8,
    stage: RwLock<String>,
    /// Progress as `f64` bits, or [`NO_PROGRESS`] when the service cannot say.
    progress: AtomicU64,
    error: RwLock<Option<(String, String)>>,
    cancelled: AtomicBool,
    events: broadcast::Sender<events::EventEnvelope>,
    result: RwLock<Option<Arc<SimulationOutput>>>,
}

/// Sentinel stored in the progress atomic while the fraction is unknown.
const NO_PROGRESS: u64 = u64::MAX;

impl Job {
    /// Creates a queued job.
    pub fn new(id: String, request: SimulationRequest, map_id: String) -> Arc<Self> {
        let (events, _) = broadcast::channel(EVENT_BACKLOG);
        let mode = request.mode_name().to_string();
        let name = request.name.clone();
        Arc::new(Self {
            id,
            name,
            map_id,
            mode,
            request: RwLock::new(request),
            created_at_ms: now_unix_ms(),
            started_at_ms: AtomicI64::new(0),
            finished_at_ms: AtomicI64::new(0),
            state: AtomicU8::new(state_byte(JobStateDto::Queued)),
            stage: RwLock::new(stage::QUEUED.to_string()),
            progress: AtomicU64::new(0f64.to_bits()),
            error: RwLock::new(None),
            cancelled: AtomicBool::new(false),
            events,
            result: RwLock::new(None),
        })
    }

    /// Identifier.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Calls `body` with the submitted request.
    ///
    /// The read lock is held for the call, so a long run keeps its payload alive and
    /// [`Job::release_payload`] waits until the worker is done with it.
    pub fn with_request<R>(&self, body: impl FnOnce(&SimulationRequest) -> R) -> Result<R> {
        let guard = self
            .request
            .read()
            .map_err(|_| ServiceError::Internal("job request lock is poisoned".to_string()))?;
        Ok(body(&guard))
    }

    /// Drops an inline map image from the retained request.
    ///
    /// A finished job needs its parameters for reporting, not its payload; a client
    /// that wants to run it again has its own copy of the request.
    pub fn release_payload(&self) {
        if let Ok(mut guard) = self.request.write() {
            guard.release_payload();
        }
    }

    /// Map the run uses.
    pub fn map_id(&self) -> &str {
        &self.map_id
    }

    /// Planning mode name.
    pub fn mode(&self) -> &str {
        &self.mode
    }

    /// Subscribes to this job's events.
    pub fn subscribe(&self) -> broadcast::Receiver<events::EventEnvelope> {
        self.events.subscribe()
    }

    /// Current state.
    pub fn state(&self) -> JobStateDto {
        state_from_byte(self.state.load(Ordering::Relaxed))
    }

    /// Current stage name.
    pub fn stage_name(&self) -> String {
        self.stage
            .read()
            .map(|stage| stage.clone())
            .unwrap_or_else(|_| stage::RUNNING.to_string())
    }

    /// Progress, or `None` while the service cannot say.
    pub fn progress(&self) -> Option<f64> {
        match self.progress.load(Ordering::Relaxed) {
            NO_PROGRESS => None,
            bits => Some(f64::from_bits(bits)),
        }
    }

    /// Seconds the job has been running, absent while queued.
    pub fn elapsed_s(&self) -> Option<f64> {
        let started = self.started_at_ms.load(Ordering::Relaxed);
        if started == 0 {
            return None;
        }
        let finished = self.finished_at_ms.load(Ordering::Relaxed);
        let end = if finished == 0 {
            now_unix_ms()
        } else {
            finished
        };
        Some((end - started).max(0) as f64 / 1000.0)
    }

    /// The result, once the job succeeded.
    pub fn result(&self) -> Option<Arc<SimulationOutput>> {
        self.result.read().ok().and_then(|guard| guard.clone())
    }

    /// Whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Records the running state.
    pub fn mark_running(&self) {
        self.started_at_ms.store(now_unix_ms(), Ordering::Relaxed);
        // A running job's fraction is unknown: the simulator's run is one call.
        self.commit(JobStateDto::Running, stage::RUNNING, None);
        self.emit_state();
    }

    /// Records success.
    pub fn mark_succeeded(&self, output: Arc<SimulationOutput>) {
        self.release_payload();
        if let Ok(mut guard) = self.result.write() {
            *guard = Some(output);
        }
        self.commit(JobStateDto::Succeeded, stage::DONE, Some(1.0));
        self.emit_state();
        self.emit(events::EventDto::Done {
            state: "succeeded".to_string(),
            summary_url: format!("/api/v1/simulations/{}/summary", self.id),
        });
    }

    /// Records failure.
    pub fn mark_failed(&self, kind: &str, message: &str) {
        self.release_payload();
        if let Ok(mut guard) = self.error.write() {
            *guard = Some((kind.to_string(), message.to_string()));
        }
        self.commit(JobStateDto::Failed, stage::FAILED, None);
        self.emit_state();
        self.emit(events::EventDto::Error {
            kind: kind.to_string(),
            message: message.to_string(),
        });
    }

    /// Records cancellation.
    ///
    /// A terminal state must be accompanied by a terminal event: the streams (SSE,
    /// WebSocket, gRPC `Watch`) end when they see one, so a cancellation that only
    /// published a state snapshot would leave every watcher of that job open forever.
    /// Cancellation is reported as an error event because that is the shape the
    /// protocol already has for "this run will not produce a result", and the kind
    /// distinguishes a deliberate cancellation from a failure.
    pub fn mark_cancelled(&self) {
        self.commit(JobStateDto::Cancelled, stage::CANCELLED, None);
        self.emit_state();
        self.emit(events::EventDto::Error {
            kind: "cancelled".to_string(),
            message: "the job was cancelled".to_string(),
        });
    }

    /// Requests cancellation. Returns false when the job already finished.
    pub fn cancel(&self) -> bool {
        if self.state().is_terminal() {
            return false;
        }
        self.cancelled.store(true, Ordering::Relaxed);
        true
    }

    /// Publishes an event.
    ///
    /// A send failure only means no subscriber is listening, which is normal for a
    /// job nobody is watching; the backlog bound means a slow subscriber never
    /// blocks the producer.
    pub fn emit(&self, event: events::EventDto) {
        let _ = self.events.send(events::EventEnvelope {
            at_ms: now_unix_ms(),
            event,
        });
    }

    /// Publishes a log line.
    pub fn log(&self, level: &str, message: impl Into<String>) {
        let message = message.into();
        match level {
            "error" => tracing::error!(job = %self.id, "{message}"),
            "warn" => tracing::warn!(job = %self.id, "{message}"),
            "debug" => tracing::debug!(job = %self.id, "{message}"),
            _ => tracing::info!(job = %self.id, "{message}"),
        }
        self.emit(events::EventDto::Log {
            level: level.to_string(),
            message,
            elapsed_s: self.elapsed_s().unwrap_or(0.0),
        });
    }

    /// The wire representation of this job.
    pub fn to_dto(&self) -> SimulationStateDto {
        let error = self.error.read().ok().and_then(|guard| guard.clone());
        let started = self.started_at_ms.load(Ordering::Relaxed);
        let finished = self.finished_at_ms.load(Ordering::Relaxed);
        SimulationStateDto {
            id: self.id.clone(),
            state: self.state(),
            stage: self.stage_name(),
            progress: self.progress(),
            elapsed_s: self.elapsed_s(),
            name: self.name.clone(),
            map_id: self.map_id.clone(),
            mode: self.mode.clone(),
            error: error.as_ref().map(|(_, message)| message.clone()),
            error_kind: error.map(|(kind, _)| kind),
            created_at: unix_ms_to_rfc3339(self.created_at_ms),
            started_at: (started != 0).then(|| unix_ms_to_rfc3339(started)),
            finished_at: (finished != 0).then(|| unix_ms_to_rfc3339(finished)),
        }
    }

    /// Publishes a transition.
    ///
    /// Stage, progress and finish time are written *before* the state, which is the
    /// commit point: a reader that observes a terminal state must also observe the
    /// fields that belong to it, or it will report "succeeded" with the stage still
    /// reading "running" for a moment.
    fn commit(&self, state: JobStateDto, stage: &str, progress: Option<f64>) {
        if let Ok(mut guard) = self.stage.write() {
            *guard = stage.to_string();
        }
        self.progress.store(
            progress.map(f64::to_bits).unwrap_or(NO_PROGRESS),
            Ordering::Relaxed,
        );
        if state.is_terminal() {
            self.finished_at_ms.store(now_unix_ms(), Ordering::Relaxed);
        }
        self.state.store(state_byte(state), Ordering::Relaxed);
    }

    fn emit_state(&self) {
        self.emit(events::EventDto::State {
            state: self.state().to_string_name().to_string(),
            stage: self.stage_name(),
            progress: self.progress(),
            elapsed_s: self.elapsed_s().unwrap_or(0.0),
        });
    }
}

impl JobStateDto {
    /// Wire name of the state.
    pub fn to_string_name(self) -> &'static str {
        match self {
            JobStateDto::Queued => "queued",
            JobStateDto::Running => "running",
            JobStateDto::Succeeded => "succeeded",
            JobStateDto::Failed => "failed",
            JobStateDto::Cancelled => "cancelled",
        }
    }
}

fn state_byte(state: JobStateDto) -> u8 {
    match state {
        JobStateDto::Queued => 0,
        JobStateDto::Running => 1,
        JobStateDto::Succeeded => 2,
        JobStateDto::Failed => 3,
        JobStateDto::Cancelled => 4,
    }
}

fn state_from_byte(value: u8) -> JobStateDto {
    match value {
        1 => JobStateDto::Running,
        2 => JobStateDto::Succeeded,
        3 => JobStateDto::Failed,
        4 => JobStateDto::Cancelled,
        _ => JobStateDto::Queued,
    }
}

/// Table of jobs, newest first.
#[derive(Debug, Default)]
pub struct JobRegistry {
    jobs: RwLock<Vec<Arc<Job>>>,
    keep_results: usize,
    keep_jobs: usize,
}

impl JobRegistry {
    /// Creates a registry keeping at most `keep_results` finished results.
    pub fn new(keep_results: usize) -> Self {
        let keep_results = keep_results.max(1);
        Self {
            jobs: RwLock::new(Vec::new()),
            keep_results,
            // Entries are cheap once their result and payload are gone, but they are
            // not free and they are not bounded by anything else: a client submitting
            // in a loop would otherwise grow the table for as long as the process
            // lives.
            keep_jobs: keep_results.saturating_mul(8).max(64),
        }
    }

    /// Admits a job if the table is below `capacity` in-flight entries.
    ///
    /// The check and the insert share one write lock: done separately, two
    /// submissions arriving together could both find room and admit one job too many.
    /// The semaphore still bounds what actually *runs*, so this is about honouring the
    /// configured queue capacity rather than about overloading the CPU.
    pub fn try_admit(&self, job: Arc<Job>, capacity: usize) -> Result<()> {
        let mut jobs = self
            .jobs
            .write()
            .map_err(|_| ServiceError::Internal("job registry lock is poisoned".to_string()))?;
        let in_flight = jobs.iter().filter(|job| !job.state().is_terminal()).count();
        if in_flight >= capacity {
            return Err(ServiceError::Busy(format!(
                "the queue is full: {in_flight} job(s) in flight, capacity {capacity}"
            )));
        }
        jobs.push(job);
        self.prune(&mut jobs);
        Ok(())
    }

    /// Adds a job.
    pub fn insert(&self, job: Arc<Job>) {
        if let Ok(mut jobs) = self.jobs.write() {
            jobs.push(job);
            self.prune(&mut jobs);
        }
    }

    /// Finds a job.
    pub fn get(&self, id: &str) -> Result<Arc<Job>> {
        self.jobs
            .read()
            .map_err(|_| ServiceError::Internal("job registry lock is poisoned".to_string()))?
            .iter()
            .find(|job| job.id() == id)
            .cloned()
            .ok_or_else(|| ServiceError::not_found(format!("simulation {id}")))
    }

    /// Lists jobs, newest first.
    pub fn list(&self) -> Vec<Arc<Job>> {
        self.jobs
            .read()
            .map(|jobs| jobs.iter().rev().cloned().collect())
            .unwrap_or_default()
    }

    /// Number of jobs that are neither finished nor cancelled.
    pub fn active(&self) -> usize {
        self.jobs
            .read()
            .map(|jobs| jobs.iter().filter(|job| !job.state().is_terminal()).count())
            .unwrap_or(0)
    }

    /// Drops the oldest finished results beyond the bound.
    ///
    /// The job entry itself stays: the id keeps resolving and the client learns
    /// that the result was evicted instead of getting a 404 for a job it just ran.
    fn prune(&self, jobs: &mut Vec<Arc<Job>>) {
        self.prune_results(jobs);
        let excess = jobs.len().saturating_sub(self.keep_jobs);
        if excess == 0 {
            return;
        }
        // Only terminal entries are dropped: a queued or running job always stays,
        // whatever the bound says.
        let mut removed = 0usize;
        jobs.retain(|job| {
            if removed < excess && job.state().is_terminal() {
                removed += 1;
                return false;
            }
            true
        });
    }

    /// Drops the oldest finished results beyond the bound.
    ///
    /// A job whose result was dropped keeps resolving: the id still answers with its
    /// state, and the client learns the result was evicted instead of getting a 404
    /// for a run it just submitted.
    fn prune_results(&self, jobs: &mut [Arc<Job>]) {
        let finished = jobs.iter().filter(|job| job.state().is_terminal()).count();
        if finished <= self.keep_results {
            return;
        }
        let mut excess = finished - self.keep_results;
        for job in jobs.iter() {
            if excess == 0 {
                break;
            }
            if job.state().is_terminal() {
                if let Ok(mut guard) = job.result.write() {
                    *guard = None;
                }
                excess -= 1;
            }
        }
    }
}
