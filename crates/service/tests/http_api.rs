//! Route-level tests: status codes, paging, error bodies and the embedded page.
//!
//! These drive the router directly through `tower`'s `oneshot` instead of binding a
//! socket, so the assertions are about the API surface rather than about the
//! network stack. Facade startup is covered by `facades.rs`.

mod fixtures;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt;

use ourealis::app::AppState;
use ourealis::config::Config;

use fixtures::{map_image, temp_dir, test_config, test_state};

/// The router under test, with the web page enabled.
async fn app() -> (Router, Arc<AppState>) {
    let mut config = test_config();
    config.server.web_enabled = true;
    let state = AppState::new(config).expect("state");
    let router = ourealis::facade::http::router(Arc::clone(&state));
    (router, state)
}

/// Builds a router over an existing state.
fn router_with(state: Arc<AppState>) -> Router {
    ourealis::facade::http::router(state)
}

/// Sends a request and returns the status, headers and body.
async fn send(
    router: &Router,
    method: &str,
    path: &str,
    body: Body,
    headers: &[(&str, &str)],
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(body).expect("request builds");
    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("router answers");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
        .await
        .expect("body reads")
        .to_vec();
    (status, headers, bytes)
}

/// Sends a request with no body and parses the reply as JSON.
async fn get_json(router: &Router, path: &str) -> (StatusCode, Value) {
    let (status, bytes) = get(router, path).await;
    (status, json(&bytes))
}

/// Sends a request with no body.
async fn get(router: &Router, path: &str) -> (StatusCode, Vec<u8>) {
    let (status, _, bytes) = send(router, "GET", path, Body::empty(), &[]).await;
    (status, bytes)
}

/// Parses a JSON body, failing with the raw text when it is not JSON.
fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|error| {
        panic!(
            "expected JSON, got {:?}: {error}",
            String::from_utf8_lossy(bytes)
        )
    })
}

#[tokio::test]
async fn health_and_system_info_describe_the_service() {
    let (router, _state) = app().await;
    let (status, bytes) = get(&router, "/api/v1/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&bytes)["status"], "ok");

    let (status, bytes) = get(&router, "/api/v1/system/info").await;
    assert_eq!(status, StatusCode::OK);
    let info = json(&bytes);
    assert_eq!(info["api_version"], "v1");
    assert_eq!(info["http_enabled"], true);
    assert!(!info["version"].as_str().unwrap_or_default().is_empty());
    assert!(
        info["presets"]
            .as_array()
            .map(|list| list.len())
            .unwrap_or(0)
            >= 3
    );
}

#[tokio::test]
async fn an_unknown_api_path_is_a_json_not_found() {
    let (router, _state) = app().await;
    let (status, bytes) = get(&router, "/api/v1/nothing-here").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let body = json(&bytes);
    assert_eq!(body["error"]["kind"], "not_found");
    assert_eq!(body["error"]["status"], 404);
}

#[tokio::test]
async fn the_map_library_lifecycle_works_over_the_api() {
    let (router, _state) = app().await;

    // Empty at first.
    let (status, bytes) = get(&router, "/api/v1/maps").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&bytes)["total"], 0);

    // Upload the synthetic map as a raw body.
    let (status, _, bytes) = send(
        &router,
        "POST",
        "/api/v1/maps?name=campus",
        Body::from(map_image()),
        &[("content-type", "application/octet-stream")],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let summary = json(&bytes);
    let id = summary["id"].as_str().expect("an id").to_string();
    assert_eq!(summary["name"], "campus");
    assert_eq!(summary["source"], "import");
    assert!(summary["layer_count"].as_u64().unwrap_or(0) >= 5);
    assert!(summary["size_bytes"].as_u64().unwrap_or(0) > 0);

    // It is listed and can be fetched.
    let (_, bytes) = get(&router, "/api/v1/maps").await;
    assert_eq!(json(&bytes)["total"], 1);
    let (status, bytes) = get(&router, &format!("/api/v1/maps/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    let metadata = json(&bytes);
    assert_eq!(metadata["summary"]["id"], id.as_str());
    assert!(metadata["header"]["chunk_size"].as_u64().unwrap_or(0) > 0);
    assert_eq!(metadata["header"]["lod_count"], 3);
    assert!(metadata["layers"].as_array().map(|l| l.len()).unwrap_or(0) >= 5);
    assert!(metadata["sections"]["connectors"].as_u64().unwrap_or(0) >= 1);

    // The grid of the elevation layer (`LayerId::ELEVATION` is 0x0001) can be read.
    let (status, bytes) = get(&router, &format!("/api/v1/maps/{id}/layers/1/grid")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let grid = json(&bytes);
    assert!(grid["chunk_dim"][0].as_u64().unwrap_or(0) >= 1);
    assert!(
        !grid["chunks"][0]
            .as_array()
            .unwrap_or(&Vec::new())
            .is_empty()
    );

    // The stored image downloads byte for byte, which is what export relies on.
    let (status, headers, downloaded) = send(
        &router,
        "GET",
        &format!("/api/v1/maps/{id}/image"),
        Body::empty(),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        downloaded,
        map_image(),
        "the download must be the stored bytes"
    );
    assert!(
        headers
            .get(header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .contains(&id),
        "the download must suggest a file name"
    );
    assert_eq!(
        ourealis_map_format::Map::from_bytes(downloaded)
            .expect("the download opens")
            .header()
            .layer_count,
        summary["layer_count"].as_u64().unwrap_or(0) as u16
    );

    // Deleting removes it, and a second delete is a 404.
    let (status, _, _) = send(
        &router,
        "DELETE",
        &format!("/api/v1/maps/{id}"),
        Body::empty(),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, bytes) = get(&router, &format!("/api/v1/maps/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json(&bytes)["error"]["kind"], "not_found");
}

#[tokio::test]
async fn a_corrupt_image_is_rejected_before_it_enters_the_library() {
    let (router, state) = app().await;
    let (status, _, bytes) = send(
        &router,
        "POST",
        "/api/v1/maps",
        Body::from(b"not an omf file".to_vec()),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json(&bytes)["error"]["kind"], "invalid");
    assert!(
        state.maps.list().is_empty(),
        "a rejected upload must not be stored"
    );
}

#[tokio::test]
async fn the_page_size_is_clamped_rather_than_rejected() {
    let mut config = test_config();
    config.http.max_page_size = 4;
    let state = AppState::new(config).expect("state");
    let router = ourealis::facade::http::router(Arc::clone(&state));
    for index in 0..6 {
        let name = format!("map-{index}");
        let (status, _, _) = send(
            &router,
            "POST",
            &format!("/api/v1/maps?name={name}"),
            Body::from(map_image()),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let (_, bytes) = get(&router, "/api/v1/maps?limit=1000000").await;
    let body = json(&bytes);
    assert_eq!(body["total"], 6);
    assert_eq!(
        body["items"].as_array().map(|items| items.len()),
        Some(4),
        "the page is capped at the configured maximum"
    );

    let (_, bytes) = get(&router, "/api/v1/maps?offset=4&limit=4").await;
    assert_eq!(
        json(&bytes)["items"].as_array().map(|items| items.len()),
        Some(2)
    );

    // A limit of zero is treated as "the smallest page", not as an error.
    let (_, bytes) = get(&router, "/api/v1/maps?limit=0").await;
    assert_eq!(
        json(&bytes)["items"].as_array().map(|items| items.len()),
        Some(1)
    );
}

#[tokio::test]
async fn the_refused_page_parameters_are_reported_as_invalid() {
    let (router, _state) = app().await;
    let (status, bytes) = get(&router, "/api/v1/maps?limit=not-a-number").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json(&bytes)["error"]["kind"], "invalid");
}

#[tokio::test]
async fn an_upload_larger_than_the_limit_is_refused_with_413() {
    let mut config = test_config();
    config.http.max_body_mb = 1;
    let state = AppState::new(config).expect("state");
    let router = ourealis::facade::http::router(Arc::clone(&state));

    let oversized = vec![0u8; 3 * 1024 * 1024];
    let (status, _, _) = send(&router, "POST", "/api/v1/maps", Body::from(oversized), &[]).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn an_upload_within_the_configured_limit_reaches_the_handler() {
    // Two limits guard a body: the tower-http stream limiter and axum's extractor
    // limit, which defaults to 2 MiB. A configuration offering 8 MiB has to mean it
    // for the extractor as well, so a 3 MiB body must be judged on its content
    // (400, not an image) rather than refused for its size.
    let mut config = test_config();
    config.http.max_body_mb = 8;
    let state = AppState::new(config).expect("state");
    let router = ourealis::facade::http::router(Arc::clone(&state));

    let body = vec![0u8; 3 * 1024 * 1024];
    let (status, _, bytes) = send(&router, "POST", "/api/v1/maps", Body::from(body), &[]).await;
    assert_ne!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "a body under http.max_body_mb must reach the handler: {}",
        String::from_utf8_lossy(&bytes)
    );
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json(&bytes)["error"]["kind"], "invalid");

    // And the limit still bites above it.
    let oversized = vec![0u8; 9 * 1024 * 1024];
    let (status, _, _) = send(&router, "POST", "/api/v1/maps", Body::from(oversized), &[]).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn a_hostile_map_id_cannot_address_a_file_outside_the_library() {
    // Ids are decoded from the request path, so an encoded separator arrives as a
    // separator again. In disk mode the id names a file, and `join` with an absolute
    // or `..`-bearing path would leave the data directory — a delete would become an
    // arbitrary file delete, which is the worst outcome the API can have.
    let dir = temp_dir("hostile-id");
    let mut config = test_config();
    config.storage.mode = ourealis::config::StorageMode::Disk;
    config.storage.data_dir = dir.clone();
    let state = AppState::new(config).expect("state");
    let router = router_with(Arc::clone(&state));

    // A decoy that a traversal would reach: it must survive every attempt.
    let decoy = dir.join("decoy.omf");
    std::fs::write(&decoy, map_image()).expect("decoy");

    for hostile in [
        "/%2F..%2Fdecoy",
        "/..%2Fdecoy",
        "/..%5Cdecoy",
        "/%2e%2e%2Fdecoy",
        "/absolute%2Fdecoy",
    ] {
        let (status, _, bytes) = send(
            &router,
            "DELETE",
            &format!("/api/v1/maps{hostile}"),
            Body::empty(),
            &[],
        )
        .await;
        assert_ne!(
            status,
            StatusCode::NO_CONTENT,
            "deleting {hostile} must not succeed"
        );
        assert_eq!(json(&bytes)["error"]["kind"], "invalid", "for {hostile}");
        assert!(decoy.is_file(), "the decoy must survive {hostile}");
        let (status, _) = get(&router, &format!("/api/v1/maps{hostile}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "reading {hostile}");
        assert!(decoy.is_file(), "the decoy must survive reading {hostile}");
    }

    // A well-formed id that simply is not in the library is a plain 404.
    let (status, bytes) = get(&router, "/api/v1/maps/not-in-the-library").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json(&bytes)["error"]["kind"], "not_found");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_hostile_map_name_cannot_escape_the_storage_directory() {
    // A map id ends up in a file name in disk mode, so a name that walks the path is
    // the one input that must never reach the filesystem unsanitised.
    let dir = temp_dir("hostile-name");
    let mut config = test_config();
    config.storage.mode = ourealis::config::StorageMode::Disk;
    config.storage.data_dir = dir.clone();
    let state = AppState::new(config).expect("state");

    for name in [
        "../../etc/passwd",
        "..%2f..%2fescape",
        "a/b/c",
        r"C:\windows\system32",
        "name with spaces",
    ] {
        let id = state.maps.next_id(name);
        assert!(
            id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "the id {id:?} derived from {name:?} must be a plain slug"
        );
        assert!(!id.contains(".."), "the id {id:?} must not walk upwards");
    }

    // Uploading under such a name writes inside `maps/` and nothing above it.
    let (status, _, bytes) = send(
        &router_with(Arc::clone(&state)),
        "POST",
        "/api/v1/maps?name=..%2f..%2fescape",
        Body::from(map_image()),
        &[],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let id = json(&bytes)["id"].as_str().expect("id").to_string();
    let stored = dir.join("maps").join(format!("{id}.omf"));
    assert!(
        stored.is_file(),
        "the image must be stored under maps/: {}",
        stored.display()
    );
    let escaped = dir.parent().map(|parent| parent.join("escape.omf"));
    if let Some(escaped) = escaped {
        assert!(
            !escaped.exists(),
            "nothing may be written outside the data directory"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_embedded_page_is_served_with_a_spa_fallback() {
    let (router, _state) = app().await;

    let (status, headers, bytes) = send(&router, "GET", "/", Body::empty(), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .starts_with("text/html"),
        "the root must be HTML"
    );
    assert!(bytes.starts_with(b"<!doctype html") || bytes.starts_with(b"<!DOCTYPE html"));
    let etag = headers
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok())
        .expect("an ETag")
        .to_string();

    // A client-side route falls back to the same document.
    let (status, _, fallback) = send(&router, "GET", "/simulations/abc", Body::empty(), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fallback, bytes);

    // A conditional request is answered with 304 and no body.
    let (status, _, body) = send(
        &router,
        "GET",
        "/",
        Body::empty(),
        &[("if-none-match", etag.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());

    // The fallback never swallows API paths.
    let (status, _) = get(&router, "/api/v1/definitely-not-a-route").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_page_can_be_disabled_independently_of_the_api() {
    let state = test_state();
    let router = ourealis::facade::http::router(Arc::clone(&state));
    // The root is not the page any more, and an unknown route is a JSON 404.
    let (status, bytes) = get(&router, "/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json(&bytes)["error"]["kind"], "not_found");
    // The API still answers.
    let (status, _) = get(&router, "/api/v1/health").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn the_omf_inspector_accepts_an_image_and_reports_its_structure() {
    let (router, _state) = app().await;
    let (status, _, bytes) = send(
        &router,
        "POST",
        "/api/v1/omf/inspect",
        Body::from(map_image()),
        &[("content-type", "application/octet-stream")],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let structure = json(&bytes);
    assert!(structure["header"]["chunk_size"].as_u64().unwrap_or(0) > 0);
    assert!(structure["footer"]["file_len"].as_u64().unwrap_or(0) > 0);
    assert!(structure["layers"].as_array().map(|l| l.len()).unwrap_or(0) >= 5);
    assert!(
        structure["directory"]
            .as_array()
            .map(|records| records.len())
            .unwrap_or(0)
            > 0
    );

    // A file that is not an OMF image is a bad request, not a server fault.
    let (status, _, bytes) = send(
        &router,
        "POST",
        "/api/v1/omf/inspect",
        Body::from(vec![0u8; 512]),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        !json(&bytes)["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test]
async fn the_preset_endpoint_describes_every_individual_knob() {
    let (router, _state) = app().await;
    let (status, bytes) = get(&router, "/api/v1/presets").await;
    assert_eq!(status, StatusCode::OK);
    let presets = json(&bytes);
    let items = presets["items"].as_array().expect("a list");
    assert_eq!(items.len(), 3, "one entry per preset");
    let first = &items[0];
    assert!(first["params"]["target_speed"].as_f64().unwrap_or(0.0) > 0.5);
    assert!(
        first["override_fields"]
            .as_array()
            .map(|fields| fields.len())
            .unwrap_or(0)
            >= 20,
        "the form needs the accepted override names"
    );
    assert!(first["defaults"]["motion"].is_object());
}

#[tokio::test]
async fn the_storage_mode_change_is_visible_in_the_state() {
    // The disk store keeps maps between states that share a directory.
    let dir = temp_dir("disk-store");
    let mut config = test_config();
    config.storage.mode = ourealis::config::StorageMode::Disk;
    config.storage.data_dir = dir.clone();
    let state = AppState::new(config.clone()).expect("state");
    let id = "disk-map";
    let summary = ourealis::api::maps::summarise(
        id,
        &map_image(),
        "import",
        ourealis::store::created_at_ms(),
    )
    .expect("summary");
    state
        .maps
        .insert(ourealis::store::MapEntry {
            id: id.to_string(),
            name: summary.name.clone(),
            source: "import".to_string(),
            created_at_ms: ourealis::store::created_at_ms(),
            summary,
            bytes: Arc::new(map_image()),
        })
        .expect("insert");

    // A fresh state over the same directory sees it.
    let reopened = AppState::new(config).expect("state");
    let listed = reopened.maps.list();
    assert_eq!(listed.len(), 1, "the disk store reloads what it wrote");
    assert_eq!(listed[0].id, id);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_region_merge_points_the_new_feature_at_the_outline_it_sent() {
    // The wire `geom_ref` is local to the request's `outlines`, because that is all a
    // client can know: the studio draws a polygon and sends it as index 0. Read as an
    // already-combined index it would address a *stored* outline instead, and the
    // exported map would draw the wrong shape while reporting success.
    let (router, _state) = app().await;
    let image = map_image();
    let before = ourealis_map_format::Map::from_bytes(image.clone()).expect("fixture opens");
    let stored_outlines = before
        .regions()
        .expect("regions read")
        .map(|set| set.polygons().len())
        .unwrap_or(0);
    assert!(
        stored_outlines >= 1,
        "the fixture must carry outlines for the offset to matter"
    );

    let outline = [[10.0f32, 10.0], [40.0, 10.0], [40.0, 30.0], [10.0, 30.0]];
    let body = json!({
        "image_base64": fixtures::base64_encode(&image),
        "edits": {
            "regions": {
                "merge": true,
                "features": [{
                    "tag_id": 7,
                    "geom_ref": 0,
                    "p_mp": 0.75,
                    "mp_bias_m": 12.0,
                    "p_loss": 0.0,
                    "trigger_mode": "spatial_deterministic"
                }],
                "outlines": [outline]
            }
        }
    });
    let (status, _, bytes) = send(
        &router,
        "POST",
        "/api/v1/omf/edit",
        Body::from(serde_json::to_vec(&body).expect("body")),
        &[("content-type", "application/json")],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );

    let edited = ourealis_map_format::Map::from_bytes(bytes).expect("the edited image opens");
    let regions = edited
        .regions()
        .expect("regions read")
        .expect("the edit keeps the region layer");
    assert_eq!(
        regions.polygons().len(),
        stored_outlines + 1,
        "the submitted outline must be appended"
    );
    let appended = regions
        .features()
        .iter()
        .find(|feature| feature.tag_id == 7)
        .expect("the new feature is present");
    assert_eq!(
        appended.geom_ref as usize, stored_outlines,
        "the new feature must reference the outline it sent, not a stored one"
    );
    let shape = regions
        .outline(appended)
        .expect("the referenced outline exists");
    assert_eq!(shape.len(), outline.len());
    for (index, point) in outline.iter().enumerate() {
        assert!(
            (shape[index][0] - point[0]).abs() < 1e-3 && (shape[index][1] - point[1]).abs() < 1e-3,
            "outline point {index} is {:?}, expected {point:?}",
            shape[index]
        );
    }
    assert_eq!(appended.mp_mode as u8, 1, "the trigger mode must survive");

    // A reference beyond the submitted outlines is refused rather than shifted into
    // a stored one.
    let bad = json!({
        "image_base64": fixtures::base64_encode(&map_image()),
        "edits": {
            "regions": {
                "merge": true,
                "features": [{
                    "tag_id": 8, "geom_ref": 5, "p_mp": 0.1, "mp_bias_m": 5.0, "p_loss": 0.0,
                    "trigger_mode": "probabilistic"
                }],
                "outlines": [outline]
            }
        }
    });
    let (status, _, bytes) = send(
        &router,
        "POST",
        "/api/v1/omf/edit",
        Body::from(serde_json::to_vec(&bad).expect("body")),
        &[("content-type", "application/json")],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert_eq!(json(&bytes)["error"]["kind"], "invalid");
}

#[tokio::test]
async fn the_synthetic_generator_honours_the_requested_seed() {
    let (router, _state) = app().await;
    // The buildings are what the seed moves, and they are the bitmap layer
    // `HARD_FORBIDDEN` (0x2001). The elevation is a function of the fixed road and
    // hill geometry, so comparing that layer would pass whatever the seed does.
    const FORBIDDEN: u16 = 0x2001;
    let build = |seed: u64| {
        let body = serde_json::json!({
            "preset": "compact",
            "seed": seed,
            "with_kpath_library": false
        });
        let router = router.clone();
        async move {
            let (status, _, bytes) = send(
                &router,
                "POST",
                "/api/v1/maps/synthetic",
                Body::from(serde_json::to_vec(&body).expect("body")),
                &[("content-type", "application/json")],
            )
            .await;
            assert_eq!(
                status,
                StatusCode::CREATED,
                "{}",
                String::from_utf8_lossy(&bytes)
            );
            let summary = json(&bytes);
            let id = summary["id"].as_str().expect("an id").to_string();
            let (status, bytes) = get(
                &router,
                &format!("/api/v1/maps/{id}/layers/{FORBIDDEN}/grid"),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::OK,
                "{}",
                String::from_utf8_lossy(&bytes)
            );
            let levels = json(&bytes)["chunks"][0].clone();
            let mut footprint = Vec::new();
            for chunk_id in levels.as_array().expect("chunk list") {
                let chunk_id = chunk_id.as_u64().expect("chunk id");
                let (status, bytes) = get(
                    &router,
                    &format!("/api/v1/maps/{id}/layers/{FORBIDDEN}/chunks/0/{chunk_id}"),
                )
                .await;
                assert_eq!(status, StatusCode::OK);
                footprint.push(json(&bytes)["data"].clone());
            }
            assert!(
                footprint.iter().any(|chunk| chunk
                    .as_array()
                    .map(|cells| cells.iter().any(|c| c.as_f64().unwrap_or(0.0) > 0.0))
                    .unwrap_or(false)),
                "the generated map must contain forbidden cells (buildings)"
            );
            footprint
        }
    };

    let first = build(11).await;
    let repeat = build(11).await;
    let other = build(12).await;

    assert_eq!(first, repeat, "the same seed must reproduce the same map");
    assert_ne!(first, other, "a different seed must move the buildings");
}

#[tokio::test]
async fn a_chunk_request_for_a_section_layer_is_a_bad_request() {
    // Regions, vectors and graphs are one opaque payload each, not grids of cells. The
    // raster shape derived for them disagreed with the bytes stored, which the reader
    // reported as corruption and the service as a broken library — a 500 on a request
    // that was simply asking the wrong layer for a chunk.
    let (router, _state) = app().await;
    let (status, _, bytes) = send(
        &router,
        "POST",
        "/api/v1/maps?name=sections",
        Body::from(map_image()),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = json(&bytes)["id"].as_str().expect("id").to_string();

    let (status, metadata) = get_json(&router, &format!("/api/v1/maps/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    let layers = metadata["layers"].as_array().expect("layers").clone();
    let mut checked = 0usize;
    for layer in layers {
        let kind = layer["kind"].as_str().unwrap_or_default();
        if kind == "raster" || kind == "bitmap" {
            continue;
        }
        let layer_id = layer["layer_id"].as_u64().expect("layer id");
        let (status, body) = get_json(
            &router,
            &format!("/api/v1/maps/{id}/layers/{layer_id}/chunks/0/0"),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "layer {layer_id} ({kind}) must be a bad request, got {status}: {body}"
        );
        assert_eq!(body["error"]["kind"], "invalid", "layer {layer_id}");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("not a cell layer"),
            "the message must say why: {body}"
        );
        checked += 1;
    }
    // The compact fixture carries the region layer and no library/graph section, so
    // one is what "the fixture has a section layer" means here.
    assert!(
        checked >= 1,
        "the fixture must carry a section layer, found {checked}"
    );

    // A cell layer still answers.
    let (status, _) = get_json(&router, &format!("/api/v1/maps/{id}/layers/1/chunks/0/0")).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn an_unreadable_library_entry_is_reported_as_a_storage_failure() {
    // The classification only matters where the bytes come from the library rather
    // than from the request: answering 400 there would tell the caller to fix a
    // request that was never wrong. The library hands back the bytes it accepted, so
    // this is driven through the store directly — an entry whose image no longer
    // parses — and the assertion is on the classification, not on how it got that way.
    let state = test_state();
    let router = router_with(Arc::clone(&state));
    let image = map_image();
    let summary = ourealis::api::maps::summarise(
        "unreadable",
        &image,
        "import",
        ourealis::store::created_at_ms(),
    )
    .expect("summary");
    let mut damaged = image.clone();
    damaged.truncate(image.len() / 2);
    state
        .maps
        .insert(ourealis::store::MapEntry {
            id: "unreadable".to_string(),
            name: summary.name.clone(),
            source: "import".to_string(),
            created_at_ms: ourealis::store::created_at_ms(),
            summary,
            bytes: Arc::new(damaged.clone()),
        })
        .expect("insert");

    let (status, body) = get_json(&router, "/api/v1/maps/unreadable").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["error"]["kind"], "internal");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("stored but cannot be read"),
        "the message must name the cause: {body}"
    );
    // The entry is still listed: its summary was built while the image was readable.
    let (_, list) = get_json(&router, "/api/v1/maps").await;
    assert_eq!(list["total"], 1);

    // The same damaged bytes offered as an upload are the caller's problem, not the
    // service's.
    let (status, _, bytes) = send(
        &router,
        "POST",
        "/api/v1/maps?name=damaged-upload",
        Body::from(damaged),
        &[],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert_eq!(json(&bytes)["error"]["kind"], "invalid");
}

#[tokio::test]
async fn a_configuration_with_cors_lists_the_allowed_origin() {
    // The header is only sent when an origin is configured; the preflight is
    // answered by the middleware stack rather than by a handler.
    let mut config: Config = test_config();
    config.http.cors_allow_origins = vec!["http://localhost:5173".to_string()];
    let state = AppState::new(config).expect("state");
    let router = ourealis::facade::http::router(Arc::clone(&state));
    let (status, headers, _) = send(
        &router,
        "GET",
        "/api/v1/health",
        Body::empty(),
        &[("origin", "http://localhost:5173")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|value| value.to_str().ok()),
        Some("http://localhost:5173")
    );
}
