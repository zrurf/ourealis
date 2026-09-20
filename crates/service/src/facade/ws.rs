//! WebSocket transport: `GET /api/v1/simulations/{id}/ws`.
//!
//! The socket carries the same event shape as SSE in one direction and the
//! paging, cancellation and keep-alive commands of the client in the other, so a
//! browser can subscribe to progress and pull a sample channel over one
//! connection.
//!
//! Frames are JSON objects with a `type` field:
//!
//! | Direction | Frame |
//! |---|---|
//! | C→S | `{"type":"subscribe","topics":["state","log"]}` |
//! | C→S | `{"type":"fetch","channel":"truth","offset":0,"limit":20000}` |
//! | C→S | `{"type":"cancel"}` |
//! | C→S | `{"type":"ping"}` |
//! | S→C | `{"type":"state"\|"stage"\|"log"\|"done"\|"error", …}` |
//! | S→C | `{"type":"chunk","channel":"accel","offset":…,"items":[…]}` |
//! | S→C | `{"type":"pong"}` |
//!
//! One task owns the socket and every producer writes through a channel, so the
//! two directions cannot interleave inside a frame and no lock is ever held
//! across an await.

use std::sync::Arc;

use axum::Router;
use axum::extract::rejection::PathRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::get;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use crate::api::API_PREFIX;
use crate::api::dto::result::EventDto;
use crate::api::maps::path_rejection;
use crate::api::streams::{channel_len, sensor_page, truth_page};
use crate::app::AppState;
use crate::error::{Result, ServiceError};
use crate::job::Job;

/// Default number of samples one fetch returns when the client does not say.
const DEFAULT_FETCH_LIMIT: usize = 20_000;

/// Frames queued for one socket before a slow client starts losing them.
const OUTGOING_QUEUE: usize = 64;

/// The WebSocket route, merged into the job API.
pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/simulations/{id}/ws", get(upgrade))
}

/// Upgrades the connection, or fails before it does.
///
/// A job that does not exist is an ordinary JSON 404: opening a socket only to
/// close it with an error frame would make a client's failure handling depend on
/// its transport.
pub async fn upgrade(
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
    upgrade: WebSocketUpgrade,
) -> Result<Response> {
    let id = path.map_err(path_rejection)?.0;
    let job = state.jobs.get(&id)?;
    Ok(upgrade.on_upgrade(move |socket| session(state, job, socket)))
}

/// One client frame.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    /// Choose the events to forward; an empty list means every event.
    Subscribe {
        /// Topic names: `state`, `stage` or `log`.
        #[serde(default)]
        topics: Vec<String>,
    },
    /// Pull a slice of a sample channel.
    Fetch {
        /// `truth`, or a sensor channel: `gnss`, `accel`, `gyro`, `mag`, `baro`.
        channel: String,
        /// First sample of the slice.
        #[serde(default)]
        offset: usize,
        /// Number of samples wanted.
        #[serde(default = "default_fetch_limit")]
        limit: usize,
    },
    /// Cancel the job.
    Cancel,
    /// Keep-alive probe.
    Ping,
}

/// Fetch limit used when a frame does not name one.
fn default_fetch_limit() -> usize {
    DEFAULT_FETCH_LIMIT
}

/// One connection's shared state.
struct Session {
    state: Arc<AppState>,
    job: Arc<Job>,
    out: mpsc::Sender<Message>,
    topics: std::sync::RwLock<Vec<String>>,
}

/// Runs one connection until either side stops.
async fn session(state: Arc<AppState>, job: Arc<Job>, mut socket: WebSocket) {
    let (out, mut outgoing) = mpsc::channel::<Message>(OUTGOING_QUEUE);
    let session = Arc::new(Session {
        state,
        job,
        out,
        topics: std::sync::RwLock::new(Vec::new()),
    });
    let events = tokio::spawn(forward_events(Arc::clone(&session)));
    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(message)) => handle(&session, message).await,
                // A closed or broken socket ends the session; the event task is
                // aborted below, which closes the channel and stops the producers.
                Some(Err(_)) | None => break,
            },
            Some(frame) = outgoing.recv() => {
                if socket.send(frame).await.is_err() {
                    break;
                }
            }
        }
    }
    events.abort();
}

/// Handles one received frame.
async fn handle(session: &Arc<Session>, message: Message) {
    match message {
        Message::Text(text) => handle_text(session, text.as_str()).await,
        Message::Binary(bytes) => match std::str::from_utf8(&bytes) {
            Ok(text) => handle_text(session, text).await,
            Err(_) => {
                send_error(
                    session,
                    &ServiceError::Invalid("a binary frame must be UTF-8 JSON".to_string()),
                )
                .await;
            }
        },
        // A close frame, a ping or a pong needs no application-level answer:
        // axum answers a ping on its own.
        Message::Close(_) | Message::Ping(_) | Message::Pong(_) => {}
    }
}

/// Handles one decoded client frame.
async fn handle_text(session: &Arc<Session>, text: &str) {
    let frame: ClientFrame = match serde_json::from_str(text) {
        Ok(frame) => frame,
        Err(error) => {
            send_error(
                session,
                &ServiceError::Invalid(format!("frame is not a usable JSON command: {error}")),
            )
            .await;
            return;
        }
    };
    match frame {
        ClientFrame::Subscribe { topics } => {
            if let Ok(mut held) = session.topics.write() {
                *held = topics;
            }
            // The subscriber gets the current state at once instead of waiting for
            // the next change, which on a long run may be the end of the run.
            send_now(session, state_frame(&session.job));
        }
        ClientFrame::Fetch {
            channel,
            offset,
            limit,
        } => fetch(session, &channel, offset, limit).await,
        ClientFrame::Cancel => {
            if session.job.cancel() {
                send_now(
                    session,
                    json!({
                        "type": "log",
                        "level": "info",
                        "message": "cancellation requested",
                        "elapsed_s": session.job.elapsed_s().unwrap_or(0.0),
                    }),
                );
            } else {
                let state = session.job.state().to_string_name();
                send_error(
                    session,
                    &ServiceError::Conflict(format!(
                        "simulation {} already finished with state {state}",
                        session.job.id()
                    )),
                )
                .await;
            }
        }
        ClientFrame::Ping => send_now(session, json!({ "type": "pong" })),
    }
}

/// Answers one fetch with chunk frames.
async fn fetch(session: &Arc<Session>, channel: &str, offset: usize, limit: usize) {
    let output = match crate::api::simulations::finished(&session.job) {
        Ok(output) => output,
        Err(error) => {
            send_error(session, &error).await;
            return;
        }
    };
    let truth = channel == "truth";
    let total = if truth {
        output.truth.len()
    } else {
        match channel_len(&output, channel) {
            Ok(total) => total,
            Err(error) => {
                send_error(session, &error).await;
                return;
            }
        }
    };
    // The page-size cap bounds one frame whatever the client asks for, exactly as
    // it bounds an HTTP response.
    let frame_samples = session.state.config.simulation.stream_frame_samples.max(1);
    let wanted = limit.min(session.state.config.http.max_page_size);
    let mut sent = 0usize;
    loop {
        let take = (wanted - sent).min(frame_samples);
        let start = offset.saturating_add(sent);
        let items: Value = if truth {
            match serde_json::to_value(truth_page(&output, start, take).items) {
                Ok(items) => items,
                Err(error) => {
                    send_error(session, &ServiceError::Json(error)).await;
                    return;
                }
            }
        } else {
            match sensor_page(&output, channel, start, take)
                .and_then(|page| Ok(serde_json::to_value(page.items)?))
            {
                Ok(items) => items,
                Err(error) => {
                    send_error(session, &error).await;
                    return;
                }
            }
        };
        let empty = items
            .as_array()
            .map(|items| items.is_empty())
            .unwrap_or(true);
        send_now(
            session,
            json!({
                "type": "chunk",
                "channel": channel,
                "offset": start,
                "total": total,
                "items": items,
            }),
        );
        sent = sent.saturating_add(take);
        if empty || sent >= wanted || start >= total {
            return;
        }
    }
}

/// Forwards the job's events until it reaches a terminal state.
async fn forward_events(session: Arc<Session>) {
    let mut receiver = session.job.subscribe();
    session
        .out
        .send(Message::text(state_frame(&session.job).to_string()))
        .await
        .ok();
    if session.job.state().is_terminal() {
        session
            .out
            .send(Message::text(terminal_frame(&session.job).to_string()))
            .await
            .ok();
        return;
    }
    loop {
        match receiver.recv().await {
            Ok(envelope) => {
                let terminal = envelope.is_terminal();
                if accepted(&session, &envelope.event) {
                    let frame = frame_of(&envelope.event);
                    if session
                        .out
                        .send(Message::text(frame.to_string()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                if terminal {
                    return;
                }
            }
            // Progress events are idempotent, so a client that fell behind is
            // brought up to date by the current state instead of a replay.
            Err(RecvError::Lagged(_)) => {
                let frame = state_frame(&session.job);
                if session
                    .out
                    .send(Message::text(frame.to_string()))
                    .await
                    .is_err()
                {
                    return;
                }
                // The run may have ended while this socket was behind; without
                // this the socket would wait for an event the job never publishes,
                // because the job holds its broadcast sender for its whole life in
                // the registry.
                if session.job.state().is_terminal() {
                    let _ = session
                        .out
                        .send(Message::text(terminal_frame(&session.job).to_string()))
                        .await;
                    return;
                }
            }
            Err(RecvError::Closed) => return,
        }
    }
}

/// True when an event ends the stream.
fn is_terminal(event: &EventDto) -> bool {
    matches!(event, EventDto::Done { .. } | EventDto::Error { .. })
}

/// True when the client's topic filter accepts an event.
fn accepted(session: &Session, event: &EventDto) -> bool {
    if is_terminal(event) {
        // The terminal event ends the stream, so it is never filtered out: a client
        // that asked only for logs must still learn that the job is over.
        return true;
    }
    let topics = session
        .topics
        .read()
        .map(|topics| topics.clone())
        .unwrap_or_default();
    crate::job::events::accepts(&topics, event)
}

/// The current state as a frame.
fn state_frame(job: &Job) -> Value {
    frame_of(&state_event(job))
}

/// The outcome of a job that already finished.
fn terminal_frame(job: &Job) -> Value {
    let dto = job.to_dto();
    match dto.state {
        crate::api::dto::JobStateDto::Failed => frame_of(&EventDto::Error {
            kind: dto.error_kind.unwrap_or_else(|| "internal".to_string()),
            message: dto
                .error
                .unwrap_or_else(|| "the run failed without a message".to_string()),
        }),
        _ => frame_of(&EventDto::Done {
            state: dto.state.to_string_name().to_string(),
            summary_url: format!("{API_PREFIX}/simulations/{}/summary", job.id()),
        }),
    }
}

/// The current state as an event.
fn state_event(job: &Job) -> EventDto {
    let dto = job.to_dto();
    EventDto::State {
        state: dto.state.to_string_name().to_string(),
        stage: dto.stage,
        progress: dto.progress,
        elapsed_s: dto.elapsed_s.unwrap_or(0.0),
    }
}

/// Renders an event into a JSON frame, falling back to an error frame.
fn frame_of(event: &EventDto) -> Value {
    match serde_json::to_value(event) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!("an event could not be serialised: {error}");
            json!({ "type": "error", "kind": "internal", "message": "the event could not be serialised" })
        }
    }
}

/// Sends a classified failure as an error frame.
async fn send_error(session: &Arc<Session>, error: &ServiceError) {
    let frame = json!({
        "type": "error",
        "kind": error.kind().as_str(),
        "message": error.to_string(),
    });
    session
        .out
        .send(Message::text(frame.to_string()))
        .await
        .ok();
}

/// Queues a frame without waiting.
///
/// The socket owner drains this channel, so a send that waited could deadlock
/// against it: a client that stopped reading loses frames instead, which the
/// documented protocol allows because every progress event is a snapshot.
fn send_now(session: &Arc<Session>, frame: Value) {
    match session.out.try_send(Message::text(frame.to_string())) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            tracing::debug!("dropping a WebSocket frame: the client is not reading");
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {}
    }
}
