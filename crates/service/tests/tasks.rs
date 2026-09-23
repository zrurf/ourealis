//! The task surface: tickets, results, cancellation and the event stream.
//!
//! Every long operation goes through a ticket, so these tests drive the lifecycle a
//! client actually uses: submit, poll, read the result, cancel what is still running —
//! and they cover the kinds that are not runs, which the simulation suite cannot.

mod fixtures;

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use ourealis::app::{AppState, Service};

use fixtures::{map_image, test_config};

/// How long a task may take before the test gives up.
const TASK_TIMEOUT: Duration = Duration::from_secs(180);

async fn app() -> (Router, Arc<AppState>) {
    let mut config = test_config();
    config.server.web_enabled = true;
    let state = AppState::new(config).expect("state");
    let router = ourealis::facade::http::router(Arc::clone(&state));
    (router, state)
}

async fn send(router: &Router, method: &str, path: &str, body: Body) -> (StatusCode, Vec<u8>) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(body)
        .expect("request builds");
    let response = router.clone().oneshot(request).await.expect("answer");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
        .await
        .expect("body")
        .to_vec();
    (status, bytes)
}

async fn get_json(router: &Router, path: &str) -> (StatusCode, Value) {
    let (status, bytes) = send(router, "GET", path, Body::empty()).await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Submits a task and returns its ticket.
async fn submit(router: &Router, body: Value) -> String {
    let (status, bytes) = send(
        router,
        "POST",
        "/api/v1/tasks",
        Body::from(serde_json::to_vec(&body).expect("body")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice::<Value>(&bytes).expect("ticket")["id"]
        .as_str()
        .expect("a ticket")
        .to_string()
}

/// Polls a ticket until it reaches a terminal state.
async fn wait(router: &Router, id: &str) -> Value {
    let deadline = Instant::now() + TASK_TIMEOUT;
    loop {
        let (status, state) = get_json(router, &format!("/api/v1/tasks/{id}")).await;
        assert_eq!(status, StatusCode::OK, "{state}");
        if state["state"] == "succeeded"
            || state["state"] == "failed"
            || state["state"] == "cancelled"
        {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "task {id} did not finish: {state}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Imports the fixture map and returns its id.
async fn upload(router: &Router) -> String {
    let (status, bytes) = send(
        router,
        "POST",
        "/api/v1/maps?name=tasks",
        Body::from(map_image()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    serde_json::from_slice::<Value>(&bytes).expect("summary")["id"]
        .as_str()
        .expect("an id")
        .to_string()
}

/// A standard request over one map.
fn request_on(map_id: &str) -> Value {
    json!({
        "map": { "kind": "id", "id": map_id },
        "route": {
            "mode": "standard",
            "start": { "x": 40.0, "y": 60.0 },
            "goal": { "x": 250.0, "y": 150.0 }
        },
        "person": { "preset": "moderate" },
        "seed": 4242
    })
}

#[tokio::test]
async fn a_plan_is_a_ticket_whose_result_is_the_candidate_set() {
    // Planning used to be a synchronous endpoint that held the request open for the
    // seconds — or tens of seconds — the planner takes. It is a task now, so the
    // submission returns immediately and the client polls or watches.
    let (router, _state) = app().await;
    let map_id = upload(&router).await;
    let id = submit(
        &router,
        json!({ "kind": "route_plan", "request": request_on(&map_id) }),
    )
    .await;

    let (status, ticket) = get_json(&router, &format!("/api/v1/tasks/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ticket["kind"], "route_plan");
    assert_eq!(ticket["map_id"], map_id.as_str());

    let state = wait(&router, &id).await;
    assert_eq!(state["state"], "succeeded", "{state}");

    let (status, result) = get_json(&router, &format!("/api/v1/tasks/{id}/result")).await;
    assert_eq!(status, StatusCode::OK);
    // A plan carries the smoothed path and its speed limits, which is what makes it a
    // plan rather than a preview.
    let route = &result["route"];
    assert!(
        !route["candidates"]
            .as_array()
            .expect("candidates")
            .is_empty()
    );
    assert!(
        !route["path"].as_array().expect("path").is_empty(),
        "a plan must carry the smoothed path"
    );
    assert!(
        !route["speed_limit_mps"]
            .as_array()
            .expect("limits")
            .is_empty()
    );

    // A preview of the same request is a different kind and carries no profile.
    let preview = submit(
        &router,
        json!({ "kind": "route_preview", "request": request_on(&map_id) }),
    )
    .await;
    let state = wait(&router, &preview).await;
    assert_eq!(state["kind"], "route_preview");
    let (_, result) = get_json(&router, &format!("/api/v1/tasks/{preview}/result")).await;
    assert!(
        result["route"]["speed_limit_mps"]
            .as_array()
            .expect("limits")
            .is_empty()
    );
}

#[tokio::test]
async fn a_task_that_has_not_finished_has_no_result_yet() {
    // `Conflict` rather than `NotFound`: the ticket is valid, and telling a client it
    // does not exist would make it start over.
    let (router, _state) = app().await;
    let map_id = upload(&router).await;
    let id = submit(
        &router,
        json!({ "kind": "route_preview", "request": request_on(&map_id) }),
    )
    .await;
    let (status, body) = get_json(&router, &format!("/api/v1/tasks/{id}/result")).await;
    assert!(
        status == StatusCode::CONFLICT || status == StatusCode::OK,
        "a queued task either conflicts or has already finished: {status} {body}"
    );
    if status == StatusCode::CONFLICT {
        assert_eq!(body["error"]["kind"], "conflict");
    }
    wait(&router, &id).await;
}

#[tokio::test]
async fn a_synthetic_map_build_reports_the_map_it_added() {
    let (router, _state) = app().await;
    let before = get_json(&router, "/api/v1/maps").await.1["total"]
        .as_u64()
        .expect("total");

    let id = submit(
        &router,
        json!({
            "kind": "synthetic_map",
            "spec": { "preset": "compact", "seed": 5 },
            "name": "from a ticket"
        }),
    )
    .await;
    let state = wait(&router, &id).await;
    assert_eq!(state["state"], "succeeded", "{state}");

    let (status, result) = get_json(&router, &format!("/api/v1/tasks/{id}/result")).await;
    assert_eq!(status, StatusCode::OK);
    let summary = &result["map"];
    assert_eq!(summary["name"], "from a ticket");
    assert_eq!(summary["source"], "synthetic");

    // The map the ticket reports is in the library, which is the point of building it.
    let listed = get_json(&router, "/api/v1/maps").await.1;
    assert_eq!(listed["total"].as_u64().expect("total"), before + 1);
    let ids: Vec<&str> = listed["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["id"].as_str())
        .collect();
    assert!(ids.contains(&summary["id"].as_str().expect("an id")));
}

#[tokio::test]
async fn a_task_can_be_cancelled_before_it_runs() {
    // One worker, so the second submission waits while the first holds the permit.
    let mut config = test_config();
    config.simulation.max_concurrent = 1;
    config.simulation.queue_capacity = 4;
    let state = AppState::new(config).expect("state");
    let router = ourealis::facade::http::router(Arc::clone(&state));
    let map_id = upload(&router).await;

    let first = submit(
        &router,
        json!({ "kind": "route_plan", "request": request_on(&map_id) }),
    )
    .await;
    let second = submit(
        &router,
        json!({ "kind": "route_plan", "request": request_on(&map_id) }),
    )
    .await;

    let (status, _) = send(
        &router,
        "DELETE",
        &format!("/api/v1/tasks/{second}"),
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let state = wait(&router, &second).await;
    assert_eq!(state["state"], "cancelled", "{state}");

    // Cancelling a task that already ended is a conflict, not a silent success.
    wait(&router, &first).await;
    let (status, body) = send(
        &router,
        "DELETE",
        &format!("/api/v1/tasks/{first}"),
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body:?}");
}

#[tokio::test]
async fn the_task_list_can_be_filtered_by_kind() {
    let (router, _state) = app().await;
    let map_id = upload(&router).await;
    let plan = submit(
        &router,
        json!({ "kind": "route_plan", "request": request_on(&map_id) }),
    )
    .await;
    let build = submit(
        &router,
        json!({ "kind": "synthetic_map", "spec": { "preset": "compact" } }),
    )
    .await;
    wait(&router, &plan).await;
    wait(&router, &build).await;

    let (status, page) = get_json(&router, "/api/v1/tasks?kind=route_plan").await;
    assert_eq!(status, StatusCode::OK);
    let kinds: Vec<&str> = page["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["kind"].as_str())
        .collect();
    assert!(!kinds.is_empty());
    assert!(
        kinds.iter().all(|kind| *kind == "route_plan"),
        "the filter must hold: {kinds:?}"
    );

    let (_, runs) = get_json(&router, "/api/v1/tasks?kind=simulation").await;
    assert_eq!(
        runs["total"].as_u64().expect("total"),
        0,
        "no run was submitted"
    );
}

#[tokio::test]
async fn a_runs_ticket_is_readable_through_the_task_endpoints() {
    // A run is a task, so one poller can watch everything; its *data* stays on the
    // simulation endpoints because it is far too large for one response.
    let (router, _state) = app().await;
    let map_id = upload(&router).await;
    let (status, bytes) = send(
        &router,
        "POST",
        "/api/v1/simulations",
        Body::from(serde_json::to_vec(&request_on(&map_id)).expect("body")),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let run = serde_json::from_slice::<Value>(&bytes).expect("reply")["id"]
        .as_str()
        .expect("an id")
        .to_string();

    let (status, ticket) = get_json(&router, &format!("/api/v1/tasks/{run}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ticket["kind"], "simulation");

    wait(&router, &run).await;
    let (status, body) = get_json(&router, &format!("/api/v1/tasks/{run}/result")).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{body}");
    // And the run's own surface still answers.
    let (status, _) = get_json(&router, &format!("/api/v1/simulations/{run}/summary")).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn the_event_stream_ends_on_a_terminal_event() {
    // A client that watches a ticket instead of polling must be told when to stop
    // reading, whatever the outcome.
    let (router, _state) = app().await;
    let map_id = upload(&router).await;
    let id = submit(
        &router,
        json!({ "kind": "route_preview", "request": request_on(&map_id) }),
    )
    .await;
    let (status, bytes) = send(
        &router,
        "GET",
        &format!("/api/v1/tasks/{id}/events"),
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("event: state"), "{text}");
    assert!(
        text.contains("event: done") || text.contains("event: error"),
        "the stream must carry a terminal event: {text}"
    );
}

#[tokio::test]
async fn an_unknown_ticket_is_a_not_found() {
    let (router, _state) = app().await;
    for path in [
        "/api/v1/tasks/nope",
        "/api/v1/tasks/nope/result",
        "/api/v1/tasks/nope/result/ref",
    ] {
        let (status, body) = get_json(&router, path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        assert_eq!(body["error"]["kind"], "not_found");
    }
}

#[tokio::test]
async fn a_submission_naming_an_impossible_workload_is_refused_immediately() {
    // The point of a ticket is that the *work* is deferred; validation is not work, so
    // a request that cannot run is still a `400` on submission.
    let (router, _state) = app().await;
    let map_id = upload(&router).await;

    let mut request = request_on(&map_id);
    request["settings"] = json!({ "sensors": { "imu_rate_hz": 1e12 } });
    let (status, body) = send(
        &router,
        "POST",
        "/api/v1/tasks",
        Body::from(
            serde_json::to_vec(&json!({ "kind": "route_preview", "request": request }))
                .expect("body"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");

    // A spec the generator cannot rasterise is refused the same way.
    let (status, body) = send(
        &router,
        "POST",
        "/api/v1/tasks",
        Body::from(
            serde_json::to_vec(&json!({
                "kind": "synthetic_map",
                "spec": { "preset": "huge", "width_m": 3_000_000.0, "height_m": 3_000_000.0 }
            }))
            .expect("body"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");

    // A kind the schema does not know is refused by the deserialiser.
    let (status, _) = send(
        &router,
        "POST",
        "/api/v1/tasks",
        Body::from(serde_json::to_vec(&json!({ "kind": "simulation" })).expect("body")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_run_is_reachable_over_both_facades_at_once() {
    // The gRPC facade registers the task service beside the simulation one, and both
    // must answer for the same ticket: a client that submits over HTTP can watch over
    // gRPC and the other way round.
    let mut config = test_config();
    config.server.web_enabled = false;
    let service = Service::start(config).await.expect("service starts");
    let port = service.rpc_addr().expect("rpc bound").port();
    let router = ourealis::facade::http::router(Arc::clone(service.state()));

    let map_id = upload(&router).await;
    let id = submit(
        &router,
        json!({ "kind": "route_preview", "request": request_on(&map_id) }),
    )
    .await;

    let mut client = ourealis::proto::v1::task_service_client::TaskServiceClient::connect(format!(
        "http://127.0.0.1:{port}"
    ))
    .await
    .expect("connect");
    let record = client
        .get(oureais_get_request(&id))
        .await
        .expect("state")
        .into_inner();
    assert_eq!(record.id, id);
    assert_eq!(
        record.kind,
        ourealis::proto::v1::TaskKind::RoutePreview as i32
    );

    service
        .shutdown(Duration::from_secs(5))
        .await
        .expect("stop");
}

/// Request of the gRPC state call, named through a helper so the import stays local.
fn oureais_get_request(id: &str) -> ourealis::proto::v1::GetTaskRequest {
    ourealis::proto::v1::GetTaskRequest { id: id.to_string() }
}
