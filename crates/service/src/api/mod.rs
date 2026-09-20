//! Transport-neutral API layer.
//!
//! Handlers live here and return DTOs; the facades in [`crate::facade`] turn those
//! into JSON or protobuf. Splitting it this way is what keeps the two wire formats
//! from disagreeing about a resource: there is one definition of each in [`dto`],
//! and one error mapping in [`error`].
//!
//! Every HTTP route is declared by a module's `router()` and merged here, under
//! [`API_PREFIX`]. A handler's contract is uniform:
//!
//! ```ignore
//! pub async fn handler(
//!     State(state): State<Arc<AppState>>,
//!     /* axum extractors */
//! ) -> Result<Json<SomeDto>>
//! ```
//!
//! `Result` is [`crate::error::Result`], whose error already knows how to become
//! an HTTP response and a gRPC status, so a handler never builds either.

pub mod base64;
pub mod dto;
pub mod error;
pub mod maps;
pub mod omf;
pub mod presets;
pub mod routes;
pub mod simulations;
pub mod streams;
pub mod system;
pub mod time;
pub mod version;

use std::sync::Arc;

use axum::Router;

use crate::app::AppState;

pub use version::{API_PREFIX, API_VERSION};

/// Every API route, ready to be nested under [`API_PREFIX`].
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .merge(system::router())
        .merge(maps::router())
        .merge(omf::router())
        .merge(presets::router())
        .merge(routes::router())
        .merge(simulations::router())
}
