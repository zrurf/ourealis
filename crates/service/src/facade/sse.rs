//! Server-sent events: `GET /api/v1/tasks/{id}/events` and
//! `GET /api/v1/simulations/{id}/events`.
//!
//! Each frame is `event: <name>` plus the task's event envelope as `data`,
//! exactly what [`EventBus::sse_frame`](crate::task::events::EventBus::sse_frame)
//! renders: the name for clients that dispatch on the SSE field, and the
//! envelope — publication time plus the event — for clients that parse the
//! payload. Two invariants keep it simple:
//!
//! * events are **idempotent snapshots**, so a subscriber that fell behind is
//!   brought up to date by re-reading the task instead of replaying what it
//!   missed;
//! * the stream **ends** after the terminal event, so a client does not have to
//!   time out a connection that will never speak again.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::api::API_PREFIX;
use crate::api::dto::result::EventDto;
use crate::api::maps::path_rejection;
use crate::app::AppState;
use crate::error::Result;
use crate::task::Task;
use crate::task::events::EventEnvelope;

/// The SSE routes, merged into the task and simulation APIs so the endpoint table
/// is complete in one place.
///
/// Both paths serve the same stream: a run is a task, and a client watching a
/// planning ticket needs the same events a run publishes.
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/tasks/{id}/events", get(events))
        .route("/simulations/{id}/events", get(events))
}

/// Streams the events of one task.
pub async fn events(
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
) -> Result<Response> {
    let id = path.map_err(path_rejection)?.0;
    let task = state.tasks.get(&id)?;
    let (sender, receiver) = mpsc::channel::<Result<Event, Infallible>>(64);
    tokio::spawn(forward(task, sender));
    let stream = Sse::new(ReceiverStream::new(receiver));
    let keep_alive_s = state.config.http.sse_keep_alive_s;
    Ok(if keep_alive_s > 0.0 {
        stream
            .keep_alive(
                KeepAlive::new()
                    .interval(Duration::from_secs_f64(keep_alive_s))
                    .text("keep-alive"),
            )
            .into_response()
    } else {
        stream.into_response()
    })
}

/// Forwards a task's events until the task reaches a terminal state.
///
/// The subscription is taken **before** the current state is read, so the
/// snapshot cannot be newer than the stream: any event published after the
/// snapshot is delivered, and a task that was already finished is recognised by
/// its state rather than by an event that will never arrive.
async fn forward(task: Arc<Task>, sender: mpsc::Sender<Result<Event, Infallible>>) {
    let mut receiver = task.subscribe();
    if sender.send(Ok(frame(&snapshot(&task)))).await.is_err() {
        return;
    }
    if task.state().is_terminal() {
        // `snapshot` already carries the terminal event for a finished task; sending it
        // twice would show a client two identical `done` frames.
        return;
    }
    loop {
        match receiver.recv().await {
            Ok(envelope) => {
                let terminal = envelope.is_terminal();
                if sender.send(Ok(frame(&envelope))).await.is_err() {
                    return;
                }
                if terminal {
                    return;
                }
            }
            // A lagged subscriber missed events, not correctness: the current
            // state says everything the missed ones would have built up to.
            Err(RecvError::Lagged(_)) => {
                if sender.send(Ok(frame(&snapshot(&task)))).await.is_err() {
                    return;
                }
                // The run may have ended while this subscriber was behind; without
                // this the stream would wait for an event the task never publishes,
                // because the task holds its broadcast sender for its whole life in
                // the registry.
                if task.state().is_terminal() {
                    let _ = sender.send(Ok(frame(&snapshot(&task)))).await;
                    return;
                }
            }
            Err(RecvError::Closed) => return,
        }
    }
}

/// Renders one envelope as an SSE frame.
fn frame(envelope: &EventEnvelope) -> Event {
    let name = envelope.name();
    match serde_json::to_string(envelope) {
        Ok(data) => Event::default().event(name).data(data),
        Err(error) => {
            tracing::warn!("an event could not be serialised: {error}");
            Event::default().event("error").data(
                r#"{"at_ms":0,"event":{"type":"error","kind":"internal","message":"the event could not be serialised"}}"#,
            )
        }
    }
}

/// An envelope around the task's current state, or around its outcome when it has
/// already finished.
fn snapshot(task: &Task) -> EventEnvelope {
    let event = if task.state().is_terminal() {
        terminal_event(task)
    } else {
        state_event(task)
    };
    EventEnvelope {
        at_ms: crate::api::time::now_unix_ms(),
        event,
    }
}

/// The current state as an event.
fn state_event(task: &Task) -> EventDto {
    let dto = task.to_dto();
    EventDto::State {
        state: dto.state.to_string_name().to_string(),
        stage: dto.stage,
        progress: dto.progress,
        elapsed_s: dto.elapsed_s.unwrap_or(0.0),
    }
}

/// The outcome of a task that already finished.
fn terminal_event(task: &Task) -> EventDto {
    let dto = task.to_dto();
    match dto.state {
        crate::api::dto::TaskState::Failed => EventDto::Error {
            kind: dto.error_kind.unwrap_or_else(|| "internal".to_string()),
            message: dto
                .error
                .unwrap_or_else(|| "the run failed without a message".to_string()),
        },
        _ => EventDto::Done {
            state: dto.state.to_string_name().to_string(),
            summary_url: format!("{API_PREFIX}/simulations/{}/summary", task.id()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest request the DTO accepts.
    fn request() -> crate::api::dto::SimulationRequest {
        serde_json::from_value(serde_json::json!({
            "route": {
                "mode": "standard",
                "start": { "x": 0.0, "y": 0.0 },
                "goal": { "x": 10.0, "y": 10.0 },
            },
        }))
        .expect("the request parses")
    }

    /// A subscriber that fell behind a run which ended without a terminal event —
    /// a cancellation only publishes a state snapshot — must still be told the run
    /// ended: the task keeps its broadcast sender for as long as it is in the
    /// registry, so nothing else would close the stream.
    #[tokio::test]
    async fn a_subscriber_that_lagged_past_the_terminal_state_ends_the_stream() {
        let task = Task::new(
            "lagged".to_string(),
            crate::task::TaskPayload::Simulation(request()),
            "map".to_string(),
            "standard".to_string(),
        );
        let (sender, mut receiver) = mpsc::channel(4);
        let forwarder = tokio::spawn(forward(Arc::clone(&task), sender));
        // Let the forwarder subscribe before the flood, so the events below are
        // missed rather than never seen; the backlog is bounded, so flooding past it
        // is what makes the subscriber lag.
        tokio::time::sleep(Duration::from_millis(20)).await;
        for index in 0..2_000 {
            task.log("info", format!("line {index}"));
        }
        task.mark_cancelled();

        let mut frames = 0usize;
        let drained = tokio::time::timeout(Duration::from_secs(5), async {
            // The channel closes only when the forwarder drops its sender.
            while receiver.recv().await.is_some() {
                frames += 1;
            }
        })
        .await;
        assert!(
            drained.is_ok(),
            "the stream stayed open after the terminal event"
        );
        assert!(
            frames >= 2,
            "the subscriber was not brought up to date: {frames} frames"
        );
        forwarder.await.expect("the forwarder finishes");
    }
}
