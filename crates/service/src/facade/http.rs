//! HTTP facade: the router, its middleware stack and the listener.
//!
//! [`router`] builds the whole application — the API under
//! [`crate::api::API_PREFIX`], the page at the root when it is enabled, and the
//! middleware that applies to both — without binding anything, so a test can
//! drive it through `tower`'s `oneshot`. [`spawn`] binds the configured address
//! and serves exactly that router, which is what keeps the tested surface and the
//! deployed one the same object.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

use crate::api::{self, API_PREFIX};
use crate::app::AppState;
use crate::error::{Result, ServiceError};
use crate::facade;

/// Request-id header used by the request-id layers and the trace span.
const REQUEST_ID: &str = "x-request-id";

/// Builds the HTTP router, without binding a socket.
///
/// Lets a test drive the API through `tower::ServiceExt::oneshot` instead of a
/// real listener; the state is already applied, so the returned router is a
/// service over `Request<Body>`.
pub fn router(state: Arc<AppState>) -> Router {
    // A nested router carries its own fallback: an unmatched path under the API
    // prefix must not fall through to the page, and it has to answer with the
    // JSON error model rather than an empty 404.
    let api = api::router().fallback(api_not_found);
    let mut app = Router::new().nest(API_PREFIX, api);
    if state.config.server.web_enabled {
        app = app.merge(super::web::router());
    }
    let app = app.fallback(root_not_found);

    // Layers wrap the router inside out: each call is outermost, so this list is
    // written from the innermost to the outermost.
    let app = app
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            request_timeout,
        ))
        .layer(RequestBodyLimitLayer::new(state.max_body_bytes()))
        // Two independent limits sit in front of a body: this streaming limiter, and
        // axum's own extractor limit, whose default is 2 MiB. Without raising the
        // second one, `http.max_body_mb` would look configurable while any upload
        // above 2 MiB was refused before the handler ran.
        .layer(DefaultBodyLimit::max(state.max_body_bytes()))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            rewrite_body_limit,
        ));
    // Compression is inserted only when the configuration asks for it, so
    // `http.compression = false` means an uncompressed response rather than a
    // setting nothing reads.
    let app = if state.config.http.compression {
        app.layer(CompressionLayer::new())
    } else {
        app
    };
    let app = app
        .layer(cors_layer(&state))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<Body>| {
                // The span is what an operator greps for, so it carries the two
                // fields that identify a request and the id that ties its lines
                // together.
                tracing::info_span!(
                    "http",
                    method = %request.method(),
                    path = %request.uri().path(),
                    request_id = request
                        .headers()
                        .get(REQUEST_ID)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or(""),
                )
            }),
        );
    app.with_state(state)
}

/// Serves the HTTP facade until the shutdown channel reports `true`.
pub async fn spawn(
    state: Arc<AppState>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<(tokio::task::JoinHandle<Result<()>>, std::net::SocketAddr)> {
    let (listener, address) = facade::bind(&state.config.server.http_listen).await?;
    let app = router(Arc::clone(&state));
    let mut shutdown = shutdown;
    let handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move { facade::wait_for_shutdown(&mut shutdown).await })
            .await
            .map_err(|error| {
                ServiceError::Internal(format!("the HTTP facade stopped serving: {error}"))
            })
    });
    Ok((handle, address))
}

/// CORS that is only sent when the configuration names an origin.
///
/// An entry that is not a usable header value is dropped with a warning rather
/// than making the service unbootable: the origin list is a deployment detail and
/// the failure is visible in the log.
fn cors_layer(state: &AppState) -> CorsLayer {
    let configured = &state.config.http.cors_allow_origins;
    let origins: Vec<header::HeaderValue> = configured
        .iter()
        .filter_map(|origin| match header::HeaderValue::from_str(origin) {
            Ok(value) => Some(value),
            Err(error) => {
                tracing::warn!("ignoring the CORS origin {origin:?}: {error}");
                None
            }
        })
        .collect();
    if origins.is_empty() {
        // No origins configured means no CORS headers at all, which is what a
        // same-origin deployment wants.
        return CorsLayer::new();
    }
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any)
}

/// Bounds the time a handler may take when the configuration asks for it.
///
/// The timeout wraps the handler's own future, not the response body: a streamed
/// result — SSE, a WebSocket or an NDJSON dump — is produced after the handler
/// has returned and is not cut off by it.
async fn request_timeout(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let seconds = state.config.http.request_timeout_s;
    if seconds <= 0.0 {
        return next.run(request).await;
    }
    match tokio::time::timeout(Duration::from_secs_f64(seconds), next.run(request)).await {
        Ok(response) => response,
        Err(_) => ServiceError::Busy(format!(
            "the request did not finish within {seconds} s (http.request_timeout_s)"
        ))
        .into_response(),
    }
}

/// Reports a body that exceeded the limit in the API's own error shape.
///
/// The body-limit layer rejects the request while the handler is still reading
/// it, which axum turns into a plain 413; rewriting it here keeps every failure a
/// client sees in one shape. A 413 that already carries JSON is left alone.
async fn rewrite_body_limit(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let response = next.run(request).await;
    if response.status() != StatusCode::PAYLOAD_TOO_LARGE || is_json(response.headers()) {
        return response;
    }
    ServiceError::TooLarge(format!(
        "the request body is larger than the configured limit of {} MiB (http.max_body_mb)",
        state.config.http.max_body_mb
    ))
    .into_response()
}

/// True when a response already carries a JSON error body.
fn is_json(headers: &header::HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.starts_with("application/json"))
        .unwrap_or(false)
}

/// Fallback of the API prefix.
async fn api_not_found(uri: axum::http::Uri) -> ServiceError {
    ServiceError::not_found(format!("no API route matches {}", uri.path()))
}

/// Fallback of the application: a JSON 404, so every answer has one shape.
async fn root_not_found(uri: axum::http::Uri) -> ServiceError {
    if uri.path() == "/" {
        ServiceError::not_found("the root document (the embedded page is disabled)")
    } else {
        ServiceError::not_found(format!("no route matches {}", uri.path()))
    }
}
