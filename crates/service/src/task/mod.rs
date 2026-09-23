//! Background tasks: state machine, event stream and execution.
//!
//! Every long operation the service performs is a task: a simulation run, a route
//! preview, a route plan, a synthetic map build. A submission returns a ticket
//! immediately and the work happens on a blocking worker, so the async reactor is
//! never held up by something that takes seconds or minutes, and a client that
//! cannot wait can watch the ticket instead of blocking on a response.
//!
//! What a client can observe is the kind, the state, the stage, the elapsed time
//! and a stream of events — the simulator's own run is a single call, so the
//! service reports *what it started* rather than inventing a percentage.

pub mod events;
pub mod queue;

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use ourealis_core::SimulationOutput;
use ourealis_map_format::synthetic::SyntheticMapSpec;
use tokio::sync::broadcast;

use crate::api::dto::{
    MapSummary, RoutePreview, SimulationRequest, TaskKindDto, TaskState, TaskStateDto,
};
use crate::api::time::{now_unix_ms, unix_ms_to_rfc3339};
use crate::error::{Result, ServiceError};

pub use events::EventBus;
pub use queue::{TaskContext, TaskRunner};

/// Stage names the service reports.
pub mod stage {
    /// Waiting for a worker slot.
    pub const QUEUED: &str = "queued";
    /// A worker holds the task and its body is running.
    pub const RUNNING: &str = "running";
    /// The task finished and its result is available.
    pub const DONE: &str = "done";
    /// The task failed.
    pub const FAILED: &str = "failed";
    /// The task was cancelled.
    pub const CANCELLED: &str = "cancelled";
}

/// Number of events a slow subscriber may fall behind before it is notified of
/// the gap rather than blocking the producer.
const EVENT_BACKLOG: usize = 1024;

/// What a task does.
#[derive(Debug, Clone)]
pub enum TaskPayload {
    /// A full run: planning, motion, sensors and metrics.
    Simulation(SimulationRequest),
    /// Planning only, stopping before the motion stage.
    Planning {
        /// The request to plan.
        request: SimulationRequest,
        /// Whether to smooth the chosen path and sample its speed limits as well.
        ///
        /// The two planning stages cost very different amounts of time, and a client
        /// editing a route needs the candidate set long before it needs the profile,
        /// so they are separate tasks rather than one long one.
        with_profile: bool,
    },
    /// Build a synthetic map and add it to the library.
    SyntheticMap {
        /// Generator settings.
        spec: SyntheticMapSpec,
        /// Name to file the map under, when the client gave one.
        name: Option<String>,
    },
}

impl TaskPayload {
    /// Kind of this payload.
    pub fn kind(&self) -> TaskKindDto {
        match self {
            TaskPayload::Simulation(_) => TaskKindDto::Simulation,
            TaskPayload::Planning {
                with_profile: false,
                ..
            } => TaskKindDto::RoutePreview,
            TaskPayload::Planning {
                with_profile: true, ..
            } => TaskKindDto::RoutePlan,
            TaskPayload::SyntheticMap { .. } => TaskKindDto::SyntheticMap,
        }
    }

    /// Drops a payload a finished task no longer needs.
    ///
    /// A simulation keeps its request while it runs — the worker reads it — and a
    /// 256 MiB inline image must not stay resident for the life of the registry once
    /// the run is over.
    fn release_payload(&mut self) {
        if let TaskPayload::Simulation(request) | TaskPayload::Planning { request, .. } = self {
            request.release_payload();
        }
    }
}

/// What a task produced.
///
/// Typed rather than a JSON blob so a caller cannot read a planning result as a run
/// digest: the accessors on [`Task`] return the variant that matches the task's kind.
#[derive(Debug)]
pub enum TaskOutcome {
    /// A finished run.
    Simulation(Arc<SimulationOutput>),
    /// A planned route.
    Route(Box<RoutePreview>),
    /// A map that was generated and added to the library.
    Map(Box<MapSummary>),
}

impl TaskOutcome {
    /// The finished run, when this outcome is one.
    ///
    /// Used by the worker for the bookkeeping that only a run needs: its summary is
    /// persisted and its size is logged, neither of which applies to a plan or a map.
    pub fn simulation(&self) -> Option<&Arc<SimulationOutput>> {
        match self {
            TaskOutcome::Simulation(output) => Some(output),
            _ => None,
        }
    }
}

/// One background task.
#[derive(Debug)]
pub struct Task {
    id: String,
    name: Option<String>,
    map_id: String,
    mode: String,
    kind: TaskKindDto,
    /// The payload as submitted. Locked rather than plain so a finished task can shed
    /// an inline image.
    payload: RwLock<TaskPayload>,
    created_at_ms: i64,
    started_at_ms: AtomicI64,
    finished_at_ms: AtomicI64,
    /// State as a byte; atomics keep it readable without a lock from the event stream
    /// and the handlers at once.
    state: AtomicU8,
    stage: RwLock<String>,
    /// Progress as `f64` bits, or [`NO_PROGRESS`] when the service cannot say.
    progress: AtomicU64,
    error: RwLock<Option<(String, String)>>,
    cancelled: AtomicBool,
    events: broadcast::Sender<events::EventEnvelope>,
    outcome: RwLock<Option<Arc<TaskOutcome>>>,
}

/// Sentinel stored in the progress atomic while the fraction is unknown.
const NO_PROGRESS: u64 = u64::MAX;

impl Task {
    /// Creates a queued task.
    ///
    /// `map_id` and `mode` are reporting labels the caller resolved from the payload;
    /// they are not re-derived here, because a synthetic map has no library id yet and
    /// a planning task's mode names the planning it was asked for.
    pub fn new(id: String, payload: TaskPayload, map_id: String, mode: String) -> Arc<Self> {
        let (events, _) = broadcast::channel(EVENT_BACKLOG);
        let name = match &payload {
            TaskPayload::Simulation(request) | TaskPayload::Planning { request, .. } => {
                request.name.clone()
            }
            TaskPayload::SyntheticMap { name, .. } => name.clone(),
        };
        let kind = payload.kind();
        Arc::new(Self {
            id,
            name,
            map_id,
            mode,
            kind,
            payload: RwLock::new(payload),
            created_at_ms: now_unix_ms(),
            started_at_ms: AtomicI64::new(0),
            finished_at_ms: AtomicI64::new(0),
            state: AtomicU8::new(state_byte(TaskState::Queued)),
            stage: RwLock::new(stage::QUEUED.to_string()),
            progress: AtomicU64::new(0f64.to_bits()),
            error: RwLock::new(None),
            cancelled: AtomicBool::new(false),
            events,
            outcome: RwLock::new(None),
        })
    }

    /// Identifier, which is also the ticket a client polls.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// What this task does.
    pub fn kind(&self) -> TaskKindDto {
        self.kind
    }

    /// Calls `body` with the payload.
    ///
    /// The read lock is held for the call, so a long run keeps its payload alive and
    /// [`Task::release_payload`] waits until the worker is done with it.
    pub fn with_payload<R>(&self, body: impl FnOnce(&TaskPayload) -> R) -> Result<R> {
        let guard = self
            .payload
            .read()
            .map_err(|_| ServiceError::Internal("task payload lock is poisoned".to_string()))?;
        Ok(body(&guard))
    }

    /// Calls `body` with the request of a simulation or planning task.
    ///
    /// Returns `None` for a kind that carries no request, which the callers of this
    /// report as "this task has no request".
    pub fn with_request<R>(&self, body: impl FnOnce(&SimulationRequest) -> R) -> Result<Option<R>> {
        self.with_payload(|payload| match payload {
            TaskPayload::Simulation(request) | TaskPayload::Planning { request, .. } => {
                Some(body(request))
            }
            TaskPayload::SyntheticMap { .. } => None,
        })
    }

    /// Drops an inline map image from the retained payload.
    ///
    /// A finished task needs its parameters for reporting, not its payload; a client
    /// that wants to run it again has its own copy of the request.
    pub fn release_payload(&self) {
        if let Ok(mut guard) = self.payload.write() {
            guard.release_payload();
        }
    }

    /// Map the task works on, as a reporting label.
    pub fn map_id(&self) -> &str {
        &self.map_id
    }

    /// Planning mode name, as a reporting label.
    pub fn mode(&self) -> &str {
        &self.mode
    }

    /// Subscribes to this task's events.
    pub fn subscribe(&self) -> broadcast::Receiver<events::EventEnvelope> {
        self.events.subscribe()
    }

    /// Current state.
    pub fn state(&self) -> TaskState {
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

    /// Seconds the task has been running, absent while queued.
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

    /// The outcome, once the task succeeded.
    pub fn outcome(&self) -> Option<Arc<TaskOutcome>> {
        self.outcome.read().ok().and_then(|guard| guard.clone())
    }

    /// The finished run, when this is a simulation that succeeded.
    pub fn simulation_result(&self) -> Option<Arc<SimulationOutput>> {
        match self.outcome()?.as_ref() {
            TaskOutcome::Simulation(output) => Some(Arc::clone(output)),
            _ => None,
        }
    }

    /// The planned route, when this is a planning task that succeeded.
    pub fn route_result(&self) -> Option<Arc<RoutePreview>> {
        match self.outcome()?.as_ref() {
            TaskOutcome::Route(preview) => Some(Arc::new((**preview).clone())),
            _ => None,
        }
    }

    /// The generated map, when this is a synthetic build that succeeded.
    pub fn map_result(&self) -> Option<Arc<MapSummary>> {
        match self.outcome()?.as_ref() {
            TaskOutcome::Map(summary) => Some(Arc::new((**summary).clone())),
            _ => None,
        }
    }

    /// Whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Records the running state.
    pub fn mark_running(&self) {
        self.started_at_ms.store(now_unix_ms(), Ordering::Relaxed);
        // A running task's fraction is unknown: its body is a single call.
        self.commit(TaskState::Running, stage::RUNNING, None);
        self.emit_state();
    }

    /// Records success.
    pub fn mark_succeeded(&self, outcome: TaskOutcome) {
        self.release_payload();
        if let Ok(mut guard) = self.outcome.write() {
            *guard = Some(Arc::new(outcome));
        }
        self.commit(TaskState::Succeeded, stage::DONE, Some(1.0));
        self.emit_state();
        self.emit(events::EventDto::Done {
            state: "succeeded".to_string(),
            summary_url: self.result_url(),
        });
    }

    /// Where a client reads this task's result.
    pub fn result_url(&self) -> String {
        match self.kind {
            TaskKindDto::Simulation => format!("/api/v1/simulations/{}/summary", self.id),
            _ => format!("/api/v1/tasks/{}/result", self.id),
        }
    }

    /// Records failure.
    pub fn mark_failed(&self, kind: &str, message: &str) {
        self.release_payload();
        if let Ok(mut guard) = self.error.write() {
            *guard = Some((kind.to_string(), message.to_string()));
        }
        self.commit(TaskState::Failed, stage::FAILED, None);
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
    /// published a state snapshot would leave every watcher of that task open forever.
    /// Cancellation is reported as an error event because that is the shape the
    /// protocol already has for "this task will not produce a result", and the kind
    /// distinguishes a deliberate cancellation from a failure.
    pub fn mark_cancelled(&self) {
        self.commit(TaskState::Cancelled, stage::CANCELLED, None);
        self.emit_state();
        self.emit(events::EventDto::Error {
            kind: "cancelled".to_string(),
            message: "the task was cancelled".to_string(),
        });
    }

    /// Requests cancellation. Returns false when the task already finished.
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
    /// task nobody is watching; the backlog bound means a slow subscriber never
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
            "error" => tracing::error!(task = %self.id, "{message}"),
            "warn" => tracing::warn!(task = %self.id, "{message}"),
            "debug" => tracing::debug!(task = %self.id, "{message}"),
            _ => tracing::info!(task = %self.id, "{message}"),
        }
        self.emit(events::EventDto::Log {
            level: level.to_string(),
            message,
            elapsed_s: self.elapsed_s().unwrap_or(0.0),
        });
    }

    /// The wire representation of this task.
    pub fn to_dto(&self) -> TaskStateDto {
        let error = self.error.read().ok().and_then(|guard| guard.clone());
        let started = self.started_at_ms.load(Ordering::Relaxed);
        let finished = self.finished_at_ms.load(Ordering::Relaxed);
        TaskStateDto {
            id: self.id.clone(),
            kind: self.kind,
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
    fn commit(&self, state: TaskState, stage: &str, progress: Option<f64>) {
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

fn state_byte(state: TaskState) -> u8 {
    match state {
        TaskState::Queued => 0,
        TaskState::Running => 1,
        TaskState::Succeeded => 2,
        TaskState::Failed => 3,
        TaskState::Cancelled => 4,
    }
}

fn state_from_byte(value: u8) -> TaskState {
    match value {
        1 => TaskState::Running,
        2 => TaskState::Succeeded,
        3 => TaskState::Failed,
        4 => TaskState::Cancelled,
        _ => TaskState::Queued,
    }
}

/// Table of tasks, newest first.
#[derive(Debug, Default)]
pub struct TaskRegistry {
    tasks: RwLock<Vec<Arc<Task>>>,
    keep_results: usize,
    keep_tasks: usize,
}

impl TaskRegistry {
    /// Creates a registry keeping at most `keep_results` finished results.
    pub fn new(keep_results: usize) -> Self {
        let keep_results = keep_results.max(1);
        Self {
            tasks: RwLock::new(Vec::new()),
            keep_results,
            // Entries are cheap once their outcome and payload are gone, but they are
            // not free and they are not bounded by anything else: a client submitting
            // in a loop would otherwise grow the table for as long as the process
            // lives.
            keep_tasks: keep_results.saturating_mul(8).max(64),
        }
    }

    /// Admits a task if the table is below `capacity` in-flight entries.
    ///
    /// The check and the insert share one write lock: done separately, two
    /// submissions arriving together could both find room and admit one too many.
    /// The semaphore still bounds what actually *runs*, so this is about honouring the
    /// configured queue capacity rather than about overloading the CPU.
    pub fn try_admit(&self, task: Arc<Task>, capacity: usize) -> Result<()> {
        let mut tasks = self
            .tasks
            .write()
            .map_err(|_| ServiceError::Internal("task registry lock is poisoned".to_string()))?;
        let in_flight = tasks
            .iter()
            .filter(|task| !task.state().is_terminal())
            .count();
        if in_flight >= capacity {
            return Err(ServiceError::Busy(format!(
                "the queue is full: {in_flight} task(s) in flight, capacity {capacity}"
            )));
        }
        tasks.push(task);
        self.prune(&mut tasks);
        Ok(())
    }

    /// Finds a task.
    pub fn get(&self, id: &str) -> Result<Arc<Task>> {
        self.tasks
            .read()
            .map_err(|_| ServiceError::Internal("task registry lock is poisoned".to_string()))?
            .iter()
            .find(|task| task.id() == id)
            .cloned()
            .ok_or_else(|| ServiceError::not_found(format!("task {id}")))
    }

    /// Lists tasks, newest first.
    pub fn list(&self) -> Vec<Arc<Task>> {
        self.tasks
            .read()
            .map(|tasks| tasks.iter().rev().cloned().collect())
            .unwrap_or_default()
    }

    /// Lists tasks of one kind, newest first.
    pub fn list_kind(&self, kind: TaskKindDto) -> Vec<Arc<Task>> {
        self.list()
            .into_iter()
            .filter(|task| task.kind() == kind)
            .collect()
    }

    /// Number of tasks that are neither finished nor cancelled.
    pub fn active(&self) -> usize {
        self.tasks
            .read()
            .map(|tasks| {
                tasks
                    .iter()
                    .filter(|task| !task.state().is_terminal())
                    .count()
            })
            .unwrap_or(0)
    }

    /// Drops the oldest finished results beyond the bound.
    ///
    /// The entry itself stays for a while: the id keeps resolving and the client
    /// learns that the result was evicted instead of getting a 404 for a task it just
    /// submitted.
    fn prune(&self, tasks: &mut Vec<Arc<Task>>) {
        self.prune_results(tasks);
        let excess = tasks.len().saturating_sub(self.keep_tasks);
        if excess == 0 {
            return;
        }
        // Only terminal entries are dropped: a queued or running task always stays,
        // whatever the bound says.
        let mut removed = 0usize;
        tasks.retain(|task| {
            if removed < excess && task.state().is_terminal() {
                removed += 1;
                return false;
            }
            true
        });
    }

    /// Drops the oldest finished results beyond the bound.
    fn prune_results(&self, tasks: &mut [Arc<Task>]) {
        // Only tasks that still hold an outcome count against the bound: a failed or
        // cancelled task occupies an entry but no memory, and counting it here would
        // evict successful results the configuration says to keep.
        let held = tasks
            .iter()
            .filter(|task| {
                task.outcome
                    .read()
                    .map(|guard| guard.is_some())
                    .unwrap_or(false)
            })
            .count();
        if held <= self.keep_results {
            return;
        }
        let mut excess = held - self.keep_results;
        for task in tasks.iter() {
            if excess == 0 {
                break;
            }
            if let Ok(mut guard) = task.outcome.write()
                && guard.is_some()
            {
                *guard = None;
                excess -= 1;
            }
        }
    }
}
