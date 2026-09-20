//! The embedded single-page application.
//!
//! The build script compiles `web/dist` into the binary, so the page and the API
//! it talks to always ship together. Two rules shape this module:
//!
//! * a **hashed** build output (anything under `assets/`) is immutable and gets
//!   the configured `Cache-Control`, while everything else — above all
//!   `index.html` — must be revalidated, or a deployment could never update a
//!   client;
//! * a client-side route that has no file behind it is answered with
//!   `index.html` when `web.spa_fallback` is on, so a reload of `/simulations/abc`
//!   works. The API prefix is never intercepted, whatever the switches say.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::get;

use crate::api::API_PREFIX;
use crate::app::AppState;
use crate::embed;
use crate::error::ServiceError;

/// Header announcing that the embedded page is the build placeholder.
const PLACEHOLDER_HEADER: HeaderName = HeaderName::from_static("x-ourealis-web");

/// The page and every file it needs.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(root))
        .route("/{*path}", get(file))
}

/// The entry point.
async fn root(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Response, ServiceError> {
    serve(&state, "index.html", &headers)
}

/// One embedded file, or the entry point for a client-side route.
async fn file(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(path): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Result<Response, ServiceError> {
    let path = path.trim_start_matches('/').to_string();
    if path.starts_with(API_PREFIX.trim_start_matches('/')) {
        // The API router is nested ahead of this one and matches first; this is a
        // guard on the invariant, not a substitute for the routing.
        return Err(ServiceError::not_found(format!(
            "{path} is under the API prefix"
        )));
    }
    if embed::get(&path).is_some() {
        return serve(&state, &path, &headers);
    }
    if state.config.web.spa_fallback {
        return serve(&state, "index.html", &headers);
    }
    Err(ServiceError::not_found(format!(
        "no page asset named {path}"
    )))
}

/// Builds the response for one embedded file.
fn serve(state: &AppState, path: &str, request: &HeaderMap) -> Result<Response, ServiceError> {
    let asset =
        embed::get(path).ok_or_else(|| ServiceError::not_found(format!("page asset {path}")))?;
    let etag = asset.etag_header();
    if is_not_modified(request, &etag) {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        insert(&mut response, header::ETAG, &etag);
        return Ok(response);
    }
    let mut response = Response::new(Body::from(asset.bytes));
    insert(&mut response, header::CONTENT_TYPE, asset.mime);
    insert(&mut response, header::ETAG, &etag);
    // Only the hashed build outputs may be cached for long: a name that carries
    // its content hash can never refer to different bytes, which is exactly what
    // the document itself cannot promise.
    let cache_control = if path.starts_with("assets/") {
        state.config.web.cache_control.clone()
    } else {
        "no-cache".to_string()
    };
    insert(&mut response, header::CACHE_CONTROL, &cache_control);
    if embed::is_placeholder() {
        insert(&mut response, PLACEHOLDER_HEADER, "placeholder");
    }
    Ok(response)
}

/// True when the request's `If-None-Match` matches the asset's tag.
fn is_not_modified(request: &HeaderMap, etag: &str) -> bool {
    let Some(value) = request
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    value
        .split(',')
        .map(str::trim)
        .any(|candidate| candidate == "*" || candidate.trim_start_matches("W/") == etag)
}

/// Inserts a header, ignoring a value that cannot be represented.
fn insert(response: &mut Response, name: header::HeaderName, value: &str) {
    match HeaderValue::from_str(value) {
        Ok(value) => {
            response.headers_mut().insert(name, value);
        }
        Err(error) => tracing::warn!("dropping the {name} header: {error}"),
    }
}
