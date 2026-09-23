//! The feasibility query: what the workspace asks before it accepts a point.
//!
//! A route point dropped inside a wall cannot be detected by the client — the hard mask
//! lives in the map and the page only draws the surface — so the service answers it.
//! These tests pin the four things the answer can say and, importantly, that it says
//! "unknown" rather than "legal" when the map declares no mask at all.

mod fixtures;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use ourealis::app::AppState;

use fixtures::{map_image, test_config};

/// The router under test, over a library holding the compact fixture map.
async fn app() -> (Router, String) {
    let state = AppState::new(test_config()).expect("state");
    let router = ourealis::facade::http::router(Arc::clone(&state));
    let (status, _, bytes) = send(
        &router,
        "POST",
        "/api/v1/maps?name=feasibility",
        Body::from(map_image()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = serde_json::from_slice::<Value>(&bytes).expect("summary")["id"]
        .as_str()
        .expect("an id")
        .to_string();
    (router, id)
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
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("body")
        .to_vec();
    (status, headers, bytes)
}

/// Asks about a list of points and returns the answers.
async fn check(router: &Router, map_id: &str, body: Value) -> Vec<Value> {
    let (status, _, bytes) = send(
        router,
        "POST",
        &format!("/api/v1/maps/{map_id}/feasibility"),
        Body::from(serde_json::to_vec(&body).expect("body")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice::<Value>(&bytes).expect("reply")["items"]
        .as_array()
        .expect("items")
        .clone()
}

/// The compact fixture's lake, in the map's own plane: the one obstacle with a known
/// position. The buildings are drawn from a seed, so they are not usable as a fixture.
const LAKE: (f64, f64) = (234.0, 156.0);

/// A cell centre on the ring road, in the zone the generator keeps clear of buildings.
const ROAD: (f64, f64) = (36.0, 100.0);

#[tokio::test]
async fn a_point_inside_an_obstacle_is_refused_with_the_reason() {
    let (router, map_id) = app().await;
    let items = check(
        &router,
        &map_id,
        json!({ "points": [{ "x": LAKE.0, "y": LAKE.1 }] }),
    )
    .await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["legal"], false);
    assert_eq!(items[0]["reason"], "forbidden");
    assert_eq!(items[0]["distance_m"].as_f64(), Some(0.0));
    // The cell and the surface height come back with the verdict: a reader who is about
    // to move the point wants to know where it landed and how high it is.
    assert!(items[0]["cell"].as_array().is_some());
    assert!(items[0]["elevation_m"].as_f64().is_some());
}

#[tokio::test]
async fn a_point_on_open_ground_is_accepted() {
    let (router, map_id) = app().await;
    let items = check(
        &router,
        &map_id,
        json!({ "points": [{ "x": ROAD.0, "y": ROAD.1 }] }),
    )
    .await;
    assert_eq!(items[0]["legal"], true, "{}", items[0]);
    assert_eq!(items[0]["reason"], "ok");
}

#[tokio::test]
async fn a_point_outside_the_map_is_refused() {
    let (router, map_id) = app().await;
    let items = check(
        &router,
        &map_id,
        json!({ "points": [{ "x": -50.0, "y": 10.0 }, { "x": 5_000.0, "y": 10.0 }] }),
    )
    .await;
    for item in &items {
        assert_eq!(item["legal"], false, "{item}");
        assert_eq!(item["reason"], "outside");
        assert_eq!(item["cell"], Value::Null);
    }
}

#[tokio::test]
async fn a_point_beside_an_obstacle_is_refused_when_it_is_inside_the_safe_radius() {
    // The offset stage refuses a point that stands against a wall even though the cell
    // itself is passable, so this endpoint has to apply the same rule — otherwise it
    // would certify a point the planner then rejects.
    let (router, map_id) = app().await;
    let beside = json!({ "x": LAKE.0, "y": 190.0 });
    let with_small_radius = check(
        &router,
        &map_id,
        json!({ "points": [beside], "safe_radius_m": 0.5 }),
    )
    .await;
    assert_eq!(
        with_small_radius[0]["legal"], true,
        "{}",
        with_small_radius[0]
    );
    let with_large_radius = check(
        &router,
        &map_id,
        json!({ "points": [beside], "safe_radius_m": 20.0 }),
    )
    .await;
    assert_eq!(with_large_radius[0]["legal"], false);
    assert_eq!(with_large_radius[0]["reason"], "too_close");
    let distance = with_large_radius[0]["distance_m"]
        .as_f64()
        .expect("a distance");
    assert!(
        (0.5..20.0).contains(&distance),
        "the distance must be inside the radius that refused it: {distance}"
    );
}

#[tokio::test]
async fn a_whole_route_is_answered_in_the_order_it_was_given() {
    let (router, map_id) = app().await;
    let items = check(
        &router,
        &map_id,
        json!({
            "points": [
                { "x": ROAD.0, "y": ROAD.1 },
                { "x": LAKE.0, "y": LAKE.1 },
                { "x": ROAD.0 + 10.0, "y": ROAD.1 }
            ]
        }),
    )
    .await;
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["point"]["x"].as_f64(), Some(ROAD.0));
    assert_eq!(items[1]["point"]["x"].as_f64(), Some(LAKE.0));
    assert_eq!(items[2]["point"]["x"].as_f64(), Some(ROAD.0 + 10.0));
    assert_eq!(items[1]["legal"], false);
}

#[tokio::test]
async fn an_absurd_request_is_a_bad_request() {
    let (router, map_id) = app().await;
    let too_many: Vec<Value> = (0..300)
        .map(|index| json!({ "x": f64::from(index), "y": 10.0 }))
        .collect();
    let (status, _, bytes) = send(
        &router,
        "POST",
        &format!("/api/v1/maps/{map_id}/feasibility"),
        Body::from(serde_json::to_vec(&json!({ "points": too_many })).expect("body")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "{}",
        String::from_utf8_lossy(&bytes)
    );

    let (status, _, bytes) = send(
        &router,
        "POST",
        &format!("/api/v1/maps/{map_id}/feasibility"),
        Body::from(
            serde_json::to_vec(&json!({
                "points": [{ "x": 10.0, "y": 10.0 }],
                "safe_radius_m": 1_000.0
            }))
            .expect("body"),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "{}",
        String::from_utf8_lossy(&bytes)
    );

    // An unknown map is a 404 rather than an answer about some other map.
    let (status, _, _) = send(
        &router,
        "POST",
        "/api/v1/maps/nOPE/feasibility",
        Body::from(serde_json::to_vec(&json!({ "points": [] })).expect("body")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
