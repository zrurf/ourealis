//! The task endpoints: submit, poll, cancel, read the result.
//!
//! Every long operation goes through a ticket, so a client never waits on a request
//! for the minutes a build or a plan can take. Submitting returns `202` with the
//! ticket; the state endpoint answers `queued`, `running`, `succeeded`, `failed` or
//! `cancelled`; the result endpoint answers with the kind's own result once the task
//! succeeded and with a `Conflict` while it has not.
//!
//! A simulation is a task, so `GET /tasks/{id}` also answers for a run id — that is
//! how a client that wants one poller for everything can have one. Runs are not
//! *submitted* here: their request builds a whole result surface, which the
//! simulation endpoints own.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use serde::Deserialize;

use crate::api::dto::simulation::SyntheticSpec;
use crate::api::dto::{
    Page, PageQuery, TaskKindDto, TaskReply, TaskResultDto, TaskResultRef, TaskState, TaskStateDto,
    TaskSubmit,
};
use crate::api::error::{json_rejection, query_rejection};
use crate::api::maps::NameQuery;
use crate::app::AppState;
use crate::error::{Result, ServiceError};
use crate::task::{Task, TaskOutcome};

/// Generates a synthetic map, returning a ticket rather than the map.
///
/// A named preset fixes the map's shape; the request's resolution, chunk size and
/// candidate-library switch are applied on top of it, and its seed always replaces
/// the preset's. The generator is bounded by `SyntheticMapSpec::validate`, but a
/// large map still takes tens of seconds to rasterise and encode, so the work runs
/// behind a ticket and the map appears in the library when it finishes.
pub async fn synthetic(
    State(state): State<Arc<AppState>>,
    query: Result<Query<NameQuery>, QueryRejection>,
    body: Result<Json<SyntheticSpec>, JsonRejection>,
) -> Result<(StatusCode, Json<TaskReply>)> {
    let query = query.map_err(query_rejection)?.0;
    let spec = body.map_err(json_rejection)?.0;
    let reply = submit_payload(
        &state,
        TaskSubmit::SyntheticMap {
            spec,
            name: query.name,
        },
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(reply)))
}

/// Submits a task and returns its ticket.
pub async fn submit(
    State(state): State<Arc<AppState>>,
    body: Result<Json<TaskSubmit>, JsonRejection>,
) -> Result<(StatusCode, Json<TaskReply>)> {
    let submit = body.map_err(json_rejection)?.0;
    Ok((
        StatusCode::ACCEPTED,
        Json(submit_payload(&state, submit).await?),
    ))
}

/// Admits one submission and builds its reply.
///
/// Shared by the two facades: a plan submitted over gRPC must produce the same ticket
/// shape as one submitted over HTTP, and the only way to guarantee that is one
/// implementation.
pub(crate) async fn submit_payload(state: &Arc<AppState>, submit: TaskSubmit) -> Result<TaskReply> {
    let id = crate::store::identifier(&submit_label(&submit));
    let task = match submit {
        TaskSubmit::RoutePreview { request } => {
            // Everything that does not need the map is checked before the ticket is
            // handed out, so a request that cannot run is a `400` on submission
            // rather than a failed task the client has to poll to find out about.
            crate::api::simulations::validate_request(&request)?;
            state.runner.submit_planning(id, request, false).await?
        }
        TaskSubmit::RoutePlan { request } => {
            crate::api::simulations::validate_request(&request)?;
            state.runner.submit_planning(id, request, true).await?
        }
        TaskSubmit::SyntheticMap { spec, name } => {
            state
                .runner
                .submit_synthetic_map(id, spec.to_spec()?, name)
                .await?
        }
    };
    Ok(TaskReply {
        id: task.id().to_string(),
        kind: task.kind(),
        state: task.state(),
    })
}

/// Identifier stem of a submission: the kind, so a ticket says what it is about.
fn submit_label(submit: &TaskSubmit) -> String {
    match submit {
        TaskSubmit::RoutePreview { .. } => "preview".to_string(),
        TaskSubmit::RoutePlan { .. } => "plan".to_string(),
        TaskSubmit::SyntheticMap { spec, .. } => {
            format!("synthetic-{:.0}m", spec.width_m.max(1.0))
        }
    }
}

/// One task's state.
pub async fn get_state(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<TaskStateDto>> {
    Ok(Json(state.tasks.get(&id)?.to_dto()))
}

/// Tasks, newest first, optionally filtered by kind.
pub async fn list(
    State(state): State<Arc<AppState>>,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Page<TaskStateDto>>> {
    let query = query.map_err(query_rejection)?.0;
    let (offset, limit) = state.page(PageQuery {
        offset: query.offset,
        limit: query
            .limit
            .unwrap_or_else(crate::api::dto::default_page_limit),
    });
    let all = match query.kind {
        Some(kind) => state.tasks.list_kind(kind),
        None => state.tasks.list(),
    };
    let total = all.len();
    let items = all
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|task| task.to_dto())
        .collect();
    Ok(Json(Page::new(items, total, offset)))
}

/// Query of the list endpoint.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ListQuery {
    /// Restrict the list to one kind.
    pub kind: Option<TaskKindDto>,
    /// Number of items to skip.
    pub offset: usize,
    /// Maximum number of items to return.
    pub limit: Option<usize>,
}

/// The result of a task that finished successfully.
///
/// A task that has not finished yet is a `Conflict` rather than a `NotFound`: the
/// ticket is valid, and telling the client it does not exist would make it start over.
pub async fn get_result(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<TaskResultDto>> {
    let task = state.tasks.get(&id)?;
    Ok(Json(result_dto(&task)?))
}

/// The result of a task, in the shape its kind defines.
///
/// Shared by the two facades for the same reason as [`submit_payload`].
pub(crate) fn result_dto(task: &Task) -> Result<TaskResultDto> {
    match task.state() {
        TaskState::Succeeded => {}
        state => {
            return Err(ServiceError::Conflict(format!(
                "task {} is {}{}",
                task.id(),
                state.to_string_name(),
                task.to_dto()
                    .error
                    .map(|message| format!(": {message}"))
                    .unwrap_or_default()
            )));
        }
    }
    match task.outcome().as_deref() {
        Some(TaskOutcome::Route(preview)) => Ok(TaskResultDto::Route(preview.clone())),
        Some(TaskOutcome::Map(summary)) => Ok(TaskResultDto::Map(summary.clone())),
        // A run's data is read from the simulation endpoints, in pages: it is far too
        // large to hand out in one response under a ticket.
        Some(TaskOutcome::Simulation(_)) => Err(ServiceError::Unsupported(format!(
            "task {} is a run: read it through /simulations/{}",
            task.id(),
            task.id()
        ))),
        None => Err(ServiceError::Conflict(format!(
            "the result of task {} was evicted from memory; submit it again",
            task.id()
        ))),
    }
}

/// Where a task's result lives, without fetching it.
pub async fn result_ref(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<TaskResultRef>> {
    let task = state.tasks.get(&id)?;
    Ok(Json(TaskResultRef {
        id: task.id().to_string(),
        kind: task.kind(),
        url: task.result_url(),
    }))
}

/// Cancels a task.
///
/// A queued task is stopped before it starts; a running one is flagged, and the flag is
/// honoured at the next boundary (a task body is a single call, so there is no point
/// inside it to stop at).
pub async fn cancel(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let task = state.tasks.get(&id)?;
    if !task.cancel() {
        return Err(ServiceError::Conflict(format!(
            "task {} is {}: it cannot be cancelled",
            task.id(),
            task.state().to_string_name()
        )));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Every route of this module.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/tasks", get(list).post(submit))
        .route("/tasks/synthetic", axum::routing::post(synthetic))
        .route("/tasks/{id}", get(get_state).delete(cancel))
        .route("/tasks/{id}/result", get(get_result))
        .route("/tasks/{id}/result/ref", get(result_ref))
}
