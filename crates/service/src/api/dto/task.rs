//! The task resource: what a client submits, what a ticket reports, what a result is.
//!
//! A task is any long operation the service performs. Submitting one returns a ticket
//! immediately (`202`), and the work happens on a blocking worker; the client reads
//! the state, watches the events or fetches the result under that ticket. Nothing on
//! this surface blocks a request for longer than the validation of its inputs.
//!
//! A simulation is a task too — `POST /simulations` returns the same kind of ticket —
//! but it has its own result surface (summary, truth, sensors, exports, audit), so its
//! submission and results live on the simulation endpoints and only its *state* is also
//! readable here.

use serde::{Deserialize, Serialize};

use super::map::MapSummary;
use super::result::RoutePreview;
use super::simulation::{SimulationRequest, SyntheticSpec};

/// Kind of work a task does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKindDto {
    /// A full run: planning, motion, sensors and metrics.
    Simulation,
    /// Planning only: the candidate set, the chosen length and the cost.
    RoutePreview,
    /// Planning plus the smoothed path and its speed limits.
    RoutePlan,
    /// Generate a synthetic map and add it to the library.
    SyntheticMap,
}

impl TaskKindDto {
    /// Whether this kind's ticket is also a simulation id.
    pub fn is_simulation(self) -> bool {
        matches!(self, TaskKindDto::Simulation)
    }
}

/// Body of `POST /api/v1/tasks`.
///
/// Tagged by `kind`, so a request that names one kind and carries another kind's body
/// is rejected by the deserialiser rather than by a hand-written check that could
/// drift from the schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskSubmit {
    /// Plan a route and report the candidate set.
    RoutePreview {
        /// The run whose route is planned. Everything but the plan is ignored.
        request: SimulationRequest,
    },
    /// Plan a route, smooth it and sample its speed limits.
    RoutePlan {
        /// The run whose route is planned.
        request: SimulationRequest,
    },
    /// Generate a synthetic map.
    SyntheticMap {
        /// Generator settings, the same shape `POST /maps/synthetic` documents.
        spec: SyntheticSpec,
        /// Name to file the map under.
        #[serde(default)]
        name: Option<String>,
    },
}

impl TaskSubmit {
    /// Kind of this submission.
    pub fn kind(&self) -> TaskKindDto {
        match self {
            TaskSubmit::RoutePreview { .. } => TaskKindDto::RoutePreview,
            TaskSubmit::RoutePlan { .. } => TaskKindDto::RoutePlan,
            TaskSubmit::SyntheticMap { .. } => TaskKindDto::SyntheticMap,
        }
    }
}

/// Reply to a submission: the ticket and its first state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskReply {
    /// Ticket the client polls, watches and cancels.
    pub id: String,
    /// Kind of the submitted task.
    pub kind: TaskKindDto,
    /// State right after submission, normally `queued`.
    pub state: TaskState,
}

/// Reply to `POST /simulations`: the ticket of a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubmitReply {
    /// Ticket of the run.
    pub id: String,
    /// State right after submission, normally `queued`.
    pub state: TaskState,
}

/// Lifecycle state of a task.
///
/// One enum for every kind, because a run is a task: a client that lists runs and a
/// client that polls a planning ticket read the same field with the same values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Waiting for a worker slot.
    Queued,
    /// Running.
    Running,
    /// Finished successfully.
    Succeeded,
    /// Finished with an error.
    Failed,
    /// Cancelled before finishing.
    Cancelled,
}

impl TaskState {
    /// True once the task cannot change state again.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TaskState::Succeeded | TaskState::Failed | TaskState::Cancelled
        )
    }
}

/// The result of a task, in the shape its kind defines.
///
/// Only one variant is ever populated, chosen by the task's kind, so a client that
/// polls a ticket it did not submit still knows what it is looking at. The JSON form
/// is externally tagged — `{"route": {...}}` or `{"map": {...}}` — which is what makes
/// the kind readable without a second field. A run is not one of them: its data is far
/// too large for a single response and is read from the simulation endpoints in pages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskResultDto {
    /// A planned route.
    Route(Box<RoutePreview>),
    /// A map that was generated and added to the library.
    Map(Box<MapSummary>),
}

impl TaskState {
    /// Wire name of the state, as the JSON and SSE payloads carry it.
    pub fn to_string_name(self) -> &'static str {
        match self {
            TaskState::Queued => "queued",
            TaskState::Running => "running",
            TaskState::Succeeded => "succeeded",
            TaskState::Failed => "failed",
            TaskState::Cancelled => "cancelled",
        }
    }
}

/// One task's state, as the state and list endpoints report it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskStateDto {
    /// Ticket.
    pub id: String,
    /// What the task does.
    pub kind: TaskKindDto,
    /// Lifecycle state.
    pub state: TaskState,
    /// Coarse stage: one of `queued`, `running`, `done`, `failed`, `cancelled`.
    pub stage: String,
    /// Fraction complete, always `null`: the service does not invent a percentage for
    /// a body that is a single call.
    pub progress: Option<f64>,
    /// Seconds since the task started, absent while queued.
    pub elapsed_s: Option<f64>,
    /// Name the client gave, for a run.
    pub name: Option<String>,
    /// Map the task works on.
    pub map_id: String,
    /// Planning mode, or the task's own label for a build.
    pub mode: String,
    /// Failure message, absent unless the task failed.
    pub error: Option<String>,
    /// Failure category, absent unless the task failed.
    pub error_kind: Option<String>,
    /// Creation time, RFC 3339.
    pub created_at: String,
    /// Start time, RFC 3339, absent while queued.
    pub started_at: Option<String>,
    /// Finish time, RFC 3339, absent while the task is live.
    pub finished_at: Option<String>,
}

/// Where a finished task's result can be read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskResultRef {
    /// Ticket the result belongs to.
    pub id: String,
    /// Kind of the task.
    pub kind: TaskKindDto,
    /// Path of the result, relative to the API prefix.
    pub url: String,
}
