//! The transports: gRPC, HTTP, WebSocket, SSE and the static page.
//!
//! Each facade is a thin encoder over the same [`crate::api`] handlers, so a
//! resource is defined once and the transports cannot drift apart. The switches
//! live in the configuration:
//!
//! * `server.rpc_enabled` — the gRPC facade on `server.rpc_listen`;
//! * `server.http_enabled` — REST, WebSocket and SSE together on
//!   `server.http_listen`, since they share a listener and a router;
//! * `server.web_enabled` — the embedded page, served by the HTTP facade on the
//!   same port. It **requires** `server.http_enabled`: a page without its API
//!   cannot work, and a configuration asking for one is refused at startup by
//!   [`crate::config::Config::validate`] rather than failing later in a browser.
//!
//! `server.rpc_enabled` and `server.http_enabled` may each be turned off; both at
//! once is refused, because nothing would be served.

pub mod http;
pub mod rpc;
pub mod sse;
pub mod web;
pub mod ws;

use std::net::SocketAddr;

use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::error::{Result, ServiceError};

/// Binds a configured `host:port` address.
///
/// The resolved address is returned alongside the listener, which is what makes
/// port 0 usable: a test (or an operator) asks the system for a free port and
/// learns which one it got.
pub async fn bind(address: &str) -> Result<(TcpListener, SocketAddr)> {
    let listener = TcpListener::bind(address).await.map_err(|error| {
        ServiceError::Internal(format!(
            "cannot bind the listener address {address}: {error}"
        ))
    })?;
    let resolved = listener.local_addr()?;
    Ok((listener, resolved))
}

/// Waits until a shutdown is requested, or until the sender is dropped.
///
/// A dropped sender means the service is going away without a signal, which is
/// also a reason to stop; blocking forever there would leave the listener open.
pub(crate) async fn wait_for_shutdown(shutdown: &mut watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    while shutdown.changed().await.is_ok() {
        if *shutdown.borrow() {
            return;
        }
    }
}
