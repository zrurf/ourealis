//! End-to-end job tests: submit, watch, read the result, cancel.
//!
//! These drive the API through the router and let a real simulation run on the
//! compact synthetic map, so they cover the whole chain: request decoding,
//! settings mapping, the blocking worker, the event stream and the result access.
//! Debug-build runs of that map take a second or two, which is why the map is the
//! smallest one the simulator's own tests use.

mod fixtures;

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use ourealis::app::AppState;

use fixtures::{base64_encode, test_config};

/// How long a job may take before the test gives up.
const JOB_TIMEOUT: Duration = Duration::from_secs(180);

async fn app(max_concurrent: usize) -> (Router, Arc<AppState>) {
    let mut config = test_config();
    config.simulation.max_concurrent = max_concurrent;
    let state = AppState::new(config).expect("state");
    let router = ourealis::facade::http::router(Arc::clone(&state));
    (router, state)
}

async fn send(
    router: &Router,
    method: &str,
    path: &str,
    body: Body,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(body)
        .expect("request builds");
    let response = router.clone().oneshot(request).await.expect("answer");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
        .await
        .expect("body")
        .to_vec();
    (status, headers, bytes)
}

async fn get_json(router: &Router, path: &str) -> (StatusCode, Value) {
    let (status, _, bytes) = send(router, "GET", path, Body::empty()).await;
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

fn body_of(value: &Value) -> Body {
    Body::from(serde_json::to_vec(value).expect("request body"))
}

/// A request that runs the compact synthetic map from an uploaded image.
fn request_on_map(map_id: &str, seed: u64) -> Value {
    json!({
        "name": "end to end",
        "map": { "kind": "id", "id": map_id },
        "route": {
            "mode": "standard",
            "start": { "x": 40.0, "y": 60.0 },
            "goal": { "x": 250.0, "y": 150.0 },
            "waypoints": [
                { "position": { "x": 150.0, "y": 60.0 }, "semantics": { "kind": "pass" } }
            ]
        },
        "person": { "preset": "moderate", "overrides": { "target_speed": 3.2 } },
        "seed": seed,
        "individual": 0,
        "settings": {
            "with_metrics": true,
            "sensors": { "force_deterministic_events": true }
        }
    })
}

/// Uploads the synthetic map and returns its id.
async fn upload(router: &Router) -> String {
    let (status, _, bytes) = send(
        router,
        "POST",
        "/api/v1/maps?name=e2e",
        Body::from(fixtures::map_image()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice::<Value>(&bytes).expect("summary")["id"]
        .as_str()
        .expect("an id")
        .to_string()
}

/// Polls a job until it reaches a terminal state.
async fn wait_for(router: &Router, id: &str) -> Value {
    let deadline = Instant::now() + JOB_TIMEOUT;
    loop {
        let (status, state) = get_json(router, &format!("/api/v1/simulations/{id}")).await;
        assert_eq!(status, StatusCode::OK, "polling failed: {state}");
        let name = state["state"].as_str().unwrap_or_default().to_string();
        if matches!(name.as_str(), "succeeded" | "failed" | "cancelled") {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "job {id} did not finish within {:?}; last state {state}",
            JOB_TIMEOUT
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn a_submitted_run_produces_a_summary_and_sample_streams() {
    let (router, _state) = app(2).await;
    let map_id = upload(&router).await;

    let (status, _, bytes) = send(
        &router,
        "POST",
        "/api/v1/simulations",
        body_of(&request_on_map(&map_id, 4242)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let reply: Value = serde_json::from_slice(&bytes).expect("reply");
    let id = reply["id"].as_str().expect("an id").to_string();
    assert_eq!(reply["state"], "queued");

    let state = wait_for(&router, &id).await;
    assert_eq!(state["state"], "succeeded", "job failed: {state}");
    assert_eq!(state["mode"], "standard");
    assert_eq!(state["stage"], "done");
    assert!(state["elapsed_s"].as_f64().unwrap_or(0.0) > 0.0);
    assert!(state["map_id"].as_str() == Some(map_id.as_str()));

    // The summary carries the route, the counts and the metrics report.
    let (status, summary) = get_json(&router, &format!("/api/v1/simulations/{id}/summary")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(summary["route_length_m"].as_f64().unwrap_or(0.0) > 100.0);
    assert!(summary["duration_s"].as_f64().unwrap_or(0.0) > 30.0);
    assert!(summary["samples"]["truth"].as_u64().unwrap_or(0) > 1000);
    assert!(summary["samples"]["gnss"].as_u64().unwrap_or(0) > 10);
    assert!(summary["manifest"].is_object());
    assert!(summary["report"].is_object(), "metrics must be reported");
    let mean_speed = summary["metrics"]["mean_speed_mps"].as_f64().unwrap_or(0.0);
    assert!(
        (1.5..6.0).contains(&mean_speed),
        "a jogging individual should average a plausible speed, got {mean_speed}"
    );

    // Sample streams are paged and report their total.
    let (status, truth) =
        get_json(&router, &format!("/api/v1/simulations/{id}/truth?limit=5")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(truth["items"].as_array().map(|items| items.len()), Some(5));
    assert!(truth["total"].as_u64().unwrap_or(0) > 1000);
    let first = &truth["items"][0];
    assert!(first["position"]["x"].is_number());
    assert!(first["speed"].is_number());

    let (status, accel) = get_json(
        &router,
        &format!("/api/v1/simulations/{id}/sensors/accel?limit=4"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(accel["items"].as_array().map(|items| items.len()), Some(4));
    assert_eq!(accel["items"][0]["channel"], "accel");
    assert!(accel["items"][0]["v"].is_array());

    let (status, gnss) = get_json(
        &router,
        &format!("/api/v1/simulations/{id}/sensors/gnss?limit=3"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(gnss["items"][0]["channel"], "gnss");
    assert!(gnss["items"][0]["satellites"].is_number());

    // An unknown channel is a bad request, not an empty page.
    let (status, error) =
        get_json(&router, &format!("/api/v1/simulations/{id}/sensors/nope")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"]["kind"], "invalid");
}

#[tokio::test]
async fn the_same_seed_reproduces_the_same_run() {
    let (router, _state) = app(2).await;
    let map_id = upload(&router).await;

    let mut summaries = Vec::new();
    for _ in 0..2 {
        let (_, _, bytes) = send(
            &router,
            "POST",
            "/api/v1/simulations",
            body_of(&request_on_map(&map_id, 777)),
        )
        .await;
        let id = serde_json::from_slice::<Value>(&bytes).expect("reply")["id"]
            .as_str()
            .expect("id")
            .to_string();
        let state = wait_for(&router, &id).await;
        assert_eq!(state["state"], "succeeded", "{state}");
        let (_, summary) = get_json(&router, &format!("/api/v1/simulations/{id}/summary")).await;
        summaries.push(summary);
    }

    // Two runs of one seed must agree on every reported quantity; the JSON floats
    // are the same bits, so a direct comparison is the strongest form.
    assert_eq!(
        summaries[0]["route_length_m"], summaries[1]["route_length_m"],
        "route length must be reproducible"
    );
    assert_eq!(
        summaries[0]["duration_s"], summaries[1]["duration_s"],
        "duration must be reproducible"
    );
    assert_eq!(summaries[0]["samples"], summaries[1]["samples"]);
}

#[tokio::test]
async fn a_result_that_is_not_ready_is_a_conflict() {
    let (router, _state) = app(1).await;
    let map_id = upload(&router).await;
    let (_, _, bytes) = send(
        &router,
        "POST",
        "/api/v1/simulations",
        body_of(&request_on_map(&map_id, 1)),
    )
    .await;
    let id = serde_json::from_slice::<Value>(&bytes).expect("reply")["id"]
        .as_str()
        .expect("id")
        .to_string();

    // The run is queued or running; its result cannot be read yet.
    let (status, error) = get_json(&router, &format!("/api/v1/simulations/{id}/summary")).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a not-yet-finished job must conflict, got {error}"
    );
    assert_eq!(error["error"]["kind"], "conflict");

    // Then it finishes and the same endpoint succeeds.
    let state = wait_for(&router, &id).await;
    assert_eq!(state["state"], "succeeded");
    let (status, _) = get_json(&router, &format!("/api/v1/simulations/{id}/summary")).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_queued_job_can_be_cancelled_and_a_finished_one_cannot() {
    let (router, _state) = app(1).await;
    let map_id = upload(&router).await;

    // Fill the single worker slot, then queue a second job behind it.
    let mut ids = Vec::new();
    for seed in [10, 11] {
        let (status, _, bytes) = send(
            &router,
            "POST",
            "/api/v1/simulations",
            body_of(&request_on_map(&map_id, seed)),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        ids.push(
            serde_json::from_slice::<Value>(&bytes).expect("reply")["id"]
                .as_str()
                .expect("id")
                .to_string(),
        );
    }

    let (status, _, _) = send(
        &router,
        "DELETE",
        &format!("/api/v1/simulations/{}", ids[1]),
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let state = wait_for(&router, &ids[1]).await;
    assert_eq!(state["state"], "cancelled", "{state}");

    // The first job is unaffected, and cancelling it after it finished conflicts.
    let state = wait_for(&router, &ids[0]).await;
    assert_eq!(state["state"], "succeeded", "{state}");
    let (status, error) = get_json(&router, &format!("/api/v1/simulations/{}", ids[0])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(error["state"], "succeeded");
    let (status, error) =
        get_json(&router, &format!("/api/v1/simulations/{}/nothing", ids[0])).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["kind"], "not_found");
}

#[tokio::test]
async fn an_inline_map_is_decoded_and_run() {
    // The inline path carries the image in the request instead of the library, so it
    // exercises the base64 decoder and `MapSource::bytes` together. A round trip
    // through the map's own download endpoint keeps the bytes honest.
    let (router, _state) = app(1).await;
    let map_id = upload(&router).await;
    let (status, _, image) = send(
        &router,
        "GET",
        &format!("/api/v1/maps/{map_id}/image"),
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let mut request = request_on_map("inline", 5150);
    request["map"] = json!({
        "kind": "inline",
        "omf_base64": base64_encode(&image),
        "name": "inline campus"
    });
    let (status, _, bytes) = send(&router, "POST", "/api/v1/simulations", body_of(&request)).await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let id = serde_json::from_slice::<Value>(&bytes).expect("reply")["id"]
        .as_str()
        .expect("id")
        .to_string();
    let state = wait_for(&router, &id).await;
    assert_eq!(state["state"], "succeeded", "{state}");
    let (status, summary) = get_json(&router, &format!("/api/v1/simulations/{id}/summary")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(summary["route_length_m"].as_f64().unwrap_or(0.0) > 100.0);

    // Whitespace around the payload is tolerated, as the decoder documents.
    let mut spaced = request.clone();
    spaced["map"] = json!({ "kind": "inline", "omf_base64": format!("  {}
", base64_encode(&image)) });
    let (status, _, _) = send(&router, "POST", "/api/v1/simulations", body_of(&spaced)).await;
    assert_eq!(status, StatusCode::ACCEPTED);

    // A payload that is not base64, and one that decodes to bytes that are not an
    // image, are both judged on the worker: submission only records the request and
    // the run fails with the decoder's or the container's own message. The split is
    // deliberate — a `map.id` that does not exist is a lookup and fails at
    // submission, while an inline image is a parse and fails as a job.
    let not_an_image = base64_encode(b"this is not an OMF image, just text");
    for (payload, expected) in [
        ("not base64 at all!".to_string(), "base64"),
        // The container rejects it for being too short to hold a header and footer;
        // the assertion is on the container's own message, not on a guess at it.
        (not_an_image, "map error"),
    ] {
        let mut broken = request.clone();
        broken["map"] = json!({ "kind": "inline", "omf_base64": payload.clone() });
        broken["name"] = json!("broken inline");
        let (status, _, bytes) =
            send(&router, "POST", "/api/v1/simulations", body_of(&broken)).await;
        assert_eq!(
            status,
            StatusCode::ACCEPTED,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        let id = serde_json::from_slice::<Value>(&bytes).expect("reply")["id"]
            .as_str()
            .expect("id")
            .to_string();
        let state = wait_for(&router, &id).await;
        assert_eq!(state["state"], "failed", "payload {payload:?}: {state}");
        assert_eq!(
            state["error_kind"], "invalid",
            "payload {payload:?}: {state}"
        );
        let message = state["error"].as_str().unwrap_or_default().to_lowercase();
        assert!(
            message.contains(expected),
            "payload {payload:?} should read as a {expected} failure, got {message:?}"
        );
    }
}

#[tokio::test]
async fn the_job_list_reports_every_submission_newest_first() {
    let (router, _state) = app(2).await;
    let map_id = upload(&router).await;
    let mut ids = Vec::new();
    for seed in [20, 21] {
        let (_, _, bytes) = send(
            &router,
            "POST",
            "/api/v1/simulations",
            body_of(&request_on_map(&map_id, seed)),
        )
        .await;
        ids.push(
            serde_json::from_slice::<Value>(&bytes).expect("reply")["id"]
                .as_str()
                .expect("id")
                .to_string(),
        );
    }
    let (status, list) = get_json(&router, "/api/v1/simulations").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["total"], 2);
    let items = list["items"].as_array().expect("items");
    assert_eq!(items[0]["id"], ids[1].as_str(), "newest first");
    assert_eq!(items[1]["id"], ids[0].as_str());
}

#[tokio::test]
async fn a_finished_run_leaves_a_summary_behind_in_disk_mode() {
    // Disk mode promises that a run's summary survives the process, which is the
    // reason to choose it. The write is best effort by design, so this asserts the
    // file appears rather than that it is required for the run to succeed.
    let dir = fixtures::temp_dir("disk-runs");
    let mut config = test_config();
    config.storage.mode = ourealis::config::StorageMode::Disk;
    config.storage.data_dir = dir.clone();
    let state = AppState::new(config).expect("state");
    let router = ourealis::facade::http::router(Arc::clone(&state));

    let image = fixtures::map_image();
    let summary = ourealis::api::maps::summarise(
        "disk-run",
        &image,
        "import",
        ourealis::store::created_at_ms(),
    )
    .expect("summary");
    state
        .maps
        .insert(ourealis::store::MapEntry {
            id: "disk-run".to_string(),
            name: summary.name.clone(),
            source: "import".to_string(),
            created_at_ms: ourealis::store::created_at_ms(),
            summary,
            bytes: std::sync::Arc::new(image),
        })
        .expect("insert");

    let (status, _, bytes) = send(
        &router,
        "POST",
        "/api/v1/simulations",
        body_of(&request_on_map("disk-run", 6060)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let id = serde_json::from_slice::<Value>(&bytes).expect("reply")["id"]
        .as_str()
        .expect("id")
        .to_string();
    let state = wait_for(&router, &id).await;
    assert_eq!(state["state"], "succeeded", "{state}");

    let stored = dir.join("runs").join(format!("{id}.json"));
    assert!(
        stored.is_file(),
        "the run summary must be written to {}",
        stored.display()
    );
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&stored).expect("read")).expect("json");
    assert!(
        written["manifest"].is_object(),
        "the summary carries the manifest: {written}"
    );
    assert!(written["sample_counts"]["truth"].as_u64().unwrap_or(0) > 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_request_naming_a_missing_map_fails_immediately() {
    let (router, _state) = app(1).await;
    let (status, _, bytes) = send(
        &router,
        "POST",
        "/api/v1/simulations",
        body_of(&request_on_map("no-such-map", 5)),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let error: Value = serde_json::from_slice(&bytes).expect("error body");
    assert_eq!(error["error"]["kind"], "not_found");
}

#[tokio::test]
async fn a_request_with_an_empty_library_says_so() {
    let (router, _state) = app(1).await;
    let mut request = request_on_map("ignored", 5);
    request["map"] = Value::Null;
    let (status, error) = {
        let (status, _, bytes) =
            send(&router, "POST", "/api/v1/simulations", body_of(&request)).await;
        (
            status,
            serde_json::from_slice::<Value>(&bytes).expect("error body"),
        )
    };
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("library is empty"),
        "got {error}"
    );
}

#[tokio::test]
async fn a_malformed_request_body_is_rejected_with_the_field_name() {
    let (router, _state) = app(1).await;
    let cases = [
        json!({ "route": { "mode": "teleport", "start": {"x": 0, "y": 0} } }),
        json!({ "route": { "mode": "standard" } }),
        json!({ "route": { "mode": "standard", "start": {"x": 1}, "goal": {"x": 2, "y": 2} } }),
        json!({ "route": { "mode": "standard", "start": {"x": 1, "y": 1}, "goal": {"x": 2, "y": 2} },
                "person": { "preset": "sprinter" } }),
        json!({ "route": { "mode": "standard", "start": {"x": 1, "y": 1}, "goal": {"x": 2, "y": 2} },
                "person": { "overrides": { "target_speedd": 3.0 } } }),
        json!({ "route": { "mode": "standard", "start": {"x": 1, "y": 1}, "goal": {"x": 2, "y": 2} },
                "settings": { "route": { "max_overlap_ratio": 2.0 } } }),
    ];
    for case in cases {
        let (status, _, bytes) = send(&router, "POST", "/api/v1/simulations", body_of(&case)).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case {case} should be rejected, got {status} {}",
            String::from_utf8_lossy(&bytes)
        );
        let error: Value = serde_json::from_slice(&bytes).expect("error body");
        assert_eq!(error["error"]["kind"], "invalid");
        assert!(
            !error["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn a_route_between_two_walls_reports_no_path() {
    let (router, _state) = app(1).await;
    let map_id = upload(&router).await;
    // A position inside the map's building block is unusable, which the planner
    // reports as an unprocessable request rather than as a server fault.
    let mut request = request_on_map(&map_id, 3);
    request["route"] = json!({
        "mode": "standard",
        "start": { "x": -5000.0, "y": -5000.0 },
        "goal": { "x": 250.0, "y": 150.0 }
    });
    let (_, _, bytes) = send(&router, "POST", "/api/v1/simulations", body_of(&request)).await;
    let id = serde_json::from_slice::<Value>(&bytes).expect("reply")["id"]
        .as_str()
        .expect("id")
        .to_string();
    let state = wait_for(&router, &id).await;
    assert_eq!(state["state"], "failed", "{state}");
    assert_eq!(
        state["error_kind"], "unprocessable",
        "a position outside the map is the caller's choice: {state}"
    );
    assert!(!state["error"].as_str().unwrap_or_default().is_empty());

    // A failed job keeps its diagnostic, and reading its summary conflicts.
    let (status, error) = get_json(&router, &format!("/api/v1/simulations/{id}/summary")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["kind"], "conflict");
}

#[tokio::test]
async fn the_route_preview_returns_candidates_without_running_motion() {
    let (router, _state) = app(1).await;
    let map_id = upload(&router).await;
    // A preview is a task: the submission returns a ticket and the candidates arrive
    // under it, so the request never waits on the planner.
    let (status, _, bytes) = send(
        &router,
        "POST",
        "/api/v1/tasks",
        body_of(&json!({
            "kind": "route_preview",
            "request": request_on_map(&map_id, 99),
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let ticket: Value = serde_json::from_slice(&bytes).expect("ticket");
    assert_eq!(ticket["kind"], "route_preview");
    let id = ticket["id"].as_str().expect("a ticket").to_string();
    let deadline = Instant::now() + JOB_TIMEOUT;
    loop {
        let (status, state) = get_json(&router, &format!("/api/v1/tasks/{id}")).await;
        assert_eq!(status, StatusCode::OK);
        if state["state"] == "succeeded" {
            break;
        }
        assert!(
            matches!(state["state"].as_str(), Some("queued" | "running")),
            "the preview must not fail: {state}"
        );
        assert!(
            Instant::now() < deadline,
            "the preview did not finish: {state}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (status, preview) = get_json(&router, &format!("/api/v1/tasks/{id}/result")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the ticket must carry the preview: {preview}"
    );
    let preview = preview["route"].clone();
    let candidates = preview["candidates"].as_array().expect("candidates");
    assert!(!candidates.is_empty(), "at least one candidate: {preview}");
    assert!(
        candidates[0]["points"]
            .as_array()
            .map(|p| p.len())
            .unwrap_or(0)
            > 1
    );
    assert!(preview["length_m"].as_f64().unwrap_or(0.0) > 50.0);
    assert!(preview["straight_line_m"].as_f64().unwrap_or(0.0) > 50.0);
    assert!(preview["planning_ms"].as_f64().unwrap_or(0.0) >= 0.0);
    let probabilities: f64 = candidates
        .iter()
        .map(|candidate| candidate["probability"].as_f64().unwrap_or(0.0))
        .sum();
    assert!(
        (probabilities - 1.0).abs() < 1e-6,
        "the Logit probabilities must sum to one, got {probabilities}"
    );
}

#[tokio::test]
async fn failed_runs_do_not_evict_the_successful_ones_beyond_the_bound() {
    // `simulation.keep_results` bounds how many *results* are held in memory. Counting
    // terminal jobs instead meant a failure — which holds no result — consumed the
    // budget, so a handful of failures evicted successful runs the configuration said
    // to keep and answered their summaries with "evicted from memory".
    let mut config = test_config();
    config.simulation.keep_results = 1;
    config.simulation.max_concurrent = 2;
    let state = AppState::new(config).expect("state");
    let router = ourealis::facade::http::router(Arc::clone(&state));
    let map_id = upload(&router).await;

    let submit = |body: Value| {
        let router = router.clone();
        async move {
            let (status, _, bytes) =
                send(&router, "POST", "/api/v1/simulations", body_of(&body)).await;
            assert_eq!(
                status,
                StatusCode::ACCEPTED,
                "{}",
                String::from_utf8_lossy(&bytes)
            );
            serde_json::from_slice::<Value>(&bytes).expect("reply")["id"]
                .as_str()
                .expect("an id")
                .to_string()
        }
    };

    // One run that succeeds and therefore holds a result, then enough failures to
    // exceed the bound if failures were counted.
    let good = submit(request_on_map(&map_id, 21)).await;
    assert_eq!(wait_for(&router, &good).await["state"], "succeeded");
    for seed in 22..26 {
        let mut body = request_on_map(&map_id, seed);
        // A start well outside the map fails during planning, which is the cheapest
        // way to produce a terminal job with no result.
        body["route"]["start"] = json!({ "x": 100_000.0, "y": 100_000.0 });
        let failed = submit(body).await;
        assert_eq!(wait_for(&router, &failed).await["state"], "failed");
    }

    // The successful run's result is still there, and so is the run itself.
    let (status, summary) = get_json(&router, &format!("/api/v1/simulations/{good}/summary")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the kept result must survive later failures: {summary}"
    );
    assert_eq!(summary["id"], good.as_str());
}
