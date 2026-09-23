//! Task event stream.
//!
//! One broadcast channel per task, fanned out to whichever transports are
//! subscribed: SSE, WebSocket and the gRPC `Watch` stream. The channel is bounded,
//! so a subscriber that stops reading is told it fell behind instead of stalling
//! the task; progress events are idempotent, so re-reading the state after a gap is
//! enough to recover.

use serde::{Deserialize, Serialize};

pub use crate::api::dto::EventDto;

/// An event with the time it was published.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    /// Publication time, Unix milliseconds.
    pub at_ms: i64,
    /// The event.
    pub event: EventDto,
}

impl EventEnvelope {
    /// Whether this event ends a watch: nothing follows it.
    pub fn is_terminal(&self) -> bool {
        matches!(self.event, EventDto::Done { .. } | EventDto::Error { .. })
    }

    /// Name of the event, matching the SSE `event:` field.
    pub fn name(&self) -> &'static str {
        self.event.name()
    }
}

/// Topics a subscriber may ask for.
pub mod topic {
    /// State changes.
    pub const STATE: &str = "state";
    /// Stage transitions.
    pub const STAGE: &str = "stage";
    /// Log lines.
    pub const LOG: &str = "log";
}

/// Whether a topic filter accepts an event.
///
/// An empty filter accepts everything, which is what a client asking for "the task"
/// without naming topics expects.
pub fn accepts(topics: &[String], event: &EventDto) -> bool {
    if topics.is_empty() {
        return true;
    }
    let name = event.name();
    topics.iter().any(|topic| topic == name)
}

/// Shared event fan-out used by the SSE, WebSocket and gRPC facades.
#[derive(Debug, Default)]
pub struct EventBus;

impl EventBus {
    /// Renders an event in the Server-Sent Events framing.
    pub fn sse_frame(envelope: &EventEnvelope) -> String {
        let data = serde_json::to_string(envelope).unwrap_or_else(|_| "{}".to_string());
        format!("event: {}\ndata: {data}\n\n", envelope.name())
    }
}
