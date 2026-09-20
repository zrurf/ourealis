//! # Ourealis service
//!
//! The simulator behind facades. Three of them, each independently switchable in
//! the configuration file:
//!
//! * **RPC** — gRPC (`tonic`), package `ourealis.api.v1`;
//! * **HTTP** — REST under `/api/v1`, plus WebSocket and SSE on the same switch;
//! * **Web** — the static single-page application, embedded in the binary, which
//!   requires the HTTP facade to be enabled.
//!
//! Everything under `api` is transport-neutral: a handler produces a DTO and an
//! error, and the facade modules turn those into JSON or protobuf. That is what
//! keeps the two sides from drifting, and it is why the protobuf messages for
//! structure-heavy payloads carry JSON: there is exactly one definition of each
//! resource, in [`api::dto`].
//!
//! Conventions this crate follows, matching the rest of the workspace:
//!
//! * every runtime output — logs, error bodies — is English;
//! * the API never invents units: metres, seconds and radians, with the unit in
//!   the field name (`*_m`, `*_s`, `*_rad`);
//! * timestamps are RFC 3339 strings in JSON and Unix milliseconds in protobuf,
//!   converted in [`api::time`];
//! * the simulator runs on a blocking worker pool; no `core` type is held across
//!   an `.await`.

#![warn(missing_docs)]

pub mod api;
pub mod app;
pub mod cli;
pub mod config;
pub mod embed;
pub mod error;
pub mod facade;
pub mod job;
pub mod store;

/// Protobuf bindings generated from `proto/` by the build script.
pub mod proto {
    /// `ourealis.api.v1` messages and services.
    ///
    /// Generated from `proto/`, so the doc comments are the ones the `.proto`
    /// files carry; fields without a comment there have none here.
    #[allow(missing_docs)]
    pub mod v1 {
        include!(concat!(env!("OUT_DIR"), "/ourealis.api.v1.rs"));
    }
}

pub use app::{AppState, Service};
pub use config::Config;
pub use error::{ErrorKind, Result, ServiceError};
