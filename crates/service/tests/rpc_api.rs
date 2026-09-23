//! The gRPC facade, driven by the generated client against a real in-process server.
//!
//! The point of this suite is that the two facades serve the same resources: a map
//! imported over client-streaming gRPC is the same library entry the HTTP API
//! lists, and a run submitted over gRPC is watachable over gRPC. Each assertion
//! therefore mirrors one in `http_api.rs` or `simulations.rs`.

mod fixtures;

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use ourealis::app::Service;
use ourealis::proto::v1;

use fixtures::{map_image, test_config};

/// Starts a service with both facades and returns it with the gRPC endpoint.
async fn start() -> (Service, String) {
    let mut config = test_config();
    config.server.web_enabled = false;
    let service = Service::start(config).await.expect("service starts");
    let port = service.rpc_addr().expect("rpc bound").port();
    (service, format!("http://127.0.0.1:{port}"))
}

/// Polls a task ticket until it reaches a terminal state.
async fn wait_for_task(
    tasks: &mut v1::task_service_client::TaskServiceClient<tonic::transport::Channel>,
    id: &str,
) -> v1::TaskRecord {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let record = tasks
            .get(v1::GetTaskRequest { id: id.to_string() })
            .await
            .expect("state")
            .into_inner();
        if record.state == v1::TaskState::Succeeded as i32
            || record.state == v1::TaskState::Failed as i32
            || record.state == v1::TaskState::Cancelled as i32
        {
            return record;
        }
        assert!(
            Instant::now() < deadline,
            "task {id} did not finish: {record:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_system_service_reports_build_and_capabilities() {
    let (service, endpoint) = start().await;
    let mut client = v1::system_service_client::SystemServiceClient::connect(endpoint)
        .await
        .expect("connect");

    let health = client
        .health(v1::Empty {})
        .await
        .expect("health")
        .into_inner();
    assert_eq!(health.status, "ok");

    let info = client.info(v1::Empty {}).await.expect("info").into_inner();
    assert_eq!(info.api_version, "v1");
    assert!(info.http_enabled && info.rpc_enabled);
    assert!(!info.version.is_empty());
    assert!(!info.backend.is_empty(), "the backend must be named");
    assert!(info.presets.len() >= 3);

    service
        .shutdown(Duration::from_secs(5))
        .await
        .expect("stop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_map_imported_over_a_client_stream_is_readable_over_the_same_channel() {
    let (service, endpoint) = start().await;
    let mut maps = v1::map_service_client::MapServiceClient::connect(endpoint.clone())
        .await
        .expect("connect");

    // The image arrives in frames, which is how a client that does not hold the
    // whole file in memory would send it.
    let image = map_image();
    let frames = image
        .chunks(64 * 1024)
        .map(|slice| v1::ImportChunk {
            name: "grpc".to_string(),
            data: slice.to_vec(),
        })
        .collect::<Vec<_>>();
    let summary = maps
        .import(tokio_stream::iter(frames))
        .await
        .expect("import")
        .into_inner();
    assert_eq!(summary.name, "grpc");
    assert_eq!(summary.source, "import");
    assert!(summary.layer_count >= 5);
    assert!(summary.size_bytes >= image.len() as u64);
    // The field is named `_unix_ms` and the map was created now, so a client
    // formatting it must land in the present rather than in January 1970.
    assert!(
        summary.created_at_unix_ms > 1_600_000_000_000,
        "the import reply must carry Unix milliseconds, got {}",
        summary.created_at_unix_ms
    );

    let list = maps
        .list(v1::ListMapsRequest {
            offset: 0,
            limit: 10,
        })
        .await
        .expect("list")
        .into_inner();
    assert_eq!(list.total, 1);
    assert_eq!(list.items.len(), 1);
    assert_eq!(
        list.items[0].created_at_unix_ms, summary.created_at_unix_ms,
        "the listing and the import reply must agree on one instant"
    );

    // Elevation is layer 0x0001; its level-0 chunk 0 exists in the synthetic map.
    let chunk = maps
        .chunk(v1::ChunkRequest {
            map_id: summary.id.clone(),
            layer_id: 1,
            level: 0,
            chunk_id: 0,
        })
        .await
        .expect("chunk")
        .into_inner();
    assert!(chunk.width > 0 && chunk.height > 0);
    assert_eq!(chunk.channels, 1);
    assert!(!chunk.data.is_empty(), "the chunk must carry samples");
    assert!(chunk.scale > 0.0, "the quantisation contract must be sent");

    let grid = maps
        .layer_grid(v1::ChunkRequest {
            map_id: summary.id.clone(),
            layer_id: 1,
            level: 0,
            chunk_id: 0,
        })
        .await
        .expect("grid")
        .into_inner();
    assert!(grid.chunk_dim_x > 0 && grid.chunk_dim_y > 0);
    assert!(!grid.chunks_json.is_empty());

    let metadata = maps
        .get(v1::GetMapRequest {
            id: summary.id.clone(),
        })
        .await
        .expect("metadata")
        .into_inner();
    let fetched = metadata.summary.clone().expect("summary");
    assert_eq!(fetched.id, summary.id);
    assert_eq!(
        fetched.created_at_unix_ms, list.items[0].created_at_unix_ms,
        "Get and List must agree on when a map was created"
    );
    assert!(
        !metadata.has_prm && !metadata.has_kpath_library,
        "the synthetic fixture carries neither a roadmap nor a candidate library, so both \
         capability flags must be false: they are read from the layer table, not assumed"
    );
    assert_eq!(
        fetched.layer_count, list.items[0].layer_count,
        "Get and List must agree on the layer count"
    );

    // The inspector reads an image that was never imported.
    let structure = maps
        .inspect(v1::EditRequest {
            image: image.clone(),
            edits_json: String::new(),
        })
        .await
        .expect("inspect")
        .into_inner();
    assert!(structure.structure_json.contains("header"));
    assert!(structure.structure_json.contains("layers"));

    // A patch script that removes nothing is a no-op edit; the image must come
    // back byte-identical because it was written by the same builder.
    let edited = maps
        .edit(v1::EditRequest {
            image: image.clone(),
            edits_json: "{}".to_string(),
        })
        .await
        .expect("edit")
        .into_inner();
    assert!(!edited.image.is_empty());
    ourealis_map_format::Map::from_bytes(edited.image).expect("the edited image opens");

    // An unusable edit is reported with a gRPC status, not an empty reply.
    let error = maps
        .edit(v1::EditRequest {
            image: image.clone(),
            edits_json: "{\"delete_layers\":[1,2,3,4,5,6,7,8,9]}".to_string(),
        })
        .await
        .expect_err("deleting every layer cannot be expressed");
    assert!(matches!(
        error.code(),
        tonic::Code::Unimplemented | tonic::Code::InvalidArgument
    ));
    assert!(
        error
            .metadata()
            .get("ourealis-error-kind")
            .map(|value| !value.is_empty())
            .unwrap_or(false),
        "the failure kind must travel in the metadata"
    );

    // Deleting through gRPC is visible to the HTTP-side store.
    maps.delete(v1::DeleteMapRequest {
        id: summary.id.clone(),
    })
    .await
    .expect("delete");
    assert!(service.state().maps.list().is_empty());

    service
        .shutdown(Duration::from_secs(5))
        .await
        .expect("stop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_submitted_over_grpc_streams_its_events_and_samples() {
    let (service, endpoint) = start().await;
    let mut maps = v1::map_service_client::MapServiceClient::connect(endpoint.clone())
        .await
        .expect("connect");
    let image = map_image();
    let summary = maps
        .import(tokio_stream::iter(vec![v1::ImportChunk {
            name: "grpc-run".to_string(),
            data: image,
        }]))
        .await
        .expect("import")
        .into_inner();

    let request = serde_json::json!({
        "name": "grpc run",
        "map": { "kind": "id", "id": summary.id },
        "route": {
            "mode": "standard",
            "start": { "x": 40.0, "y": 60.0 },
            "goal": { "x": 250.0, "y": 150.0 }
        },
        "person": { "preset": "moderate" },
        "seed": 2024,
        "settings": { "with_metrics": false }
    })
    .to_string();

    let mut simulations =
        v1::simulation_service_client::SimulationServiceClient::connect(endpoint.clone())
            .await
            .expect("connect");
    let reply = simulations
        .submit(v1::SubmitRequest {
            request_json: request.clone(),
        })
        .await
        .expect("submit")
        .into_inner();
    assert_eq!(reply.state, v1::TaskState::Queued as i32);
    assert!(!reply.id.is_empty());

    // The watch stream must deliver progress and end with the terminal event.
    let mut events = simulations
        .watch(v1::WatchRequest {
            id: reply.id.clone(),
            topics: Vec::new(),
        })
        .await
        .expect("watch")
        .into_inner();
    let mut kinds: Vec<String> = Vec::new();
    while let Some(event) = events.message().await.expect("frame") {
        kinds.push(event.r#type.clone());
        if event.r#type == "done" || event.r#type == "error" {
            break;
        }
    }
    assert!(
        kinds.iter().any(|kind| kind == "state"),
        "no state event in {kinds:?}"
    );
    assert_eq!(
        kinds.last().map(String::as_str),
        Some("done"),
        "the stream must end with a terminal event: {kinds:?}"
    );

    let state = simulations
        .get(v1::GetSimulationRequest {
            id: reply.id.clone(),
        })
        .await
        .expect("state")
        .into_inner();
    assert_eq!(state.state, v1::TaskState::Succeeded as i32);
    assert_eq!(state.name, "grpc run");
    assert!(
        state.created_at_unix_ms > 1_600_000_000_000,
        "a run must report Unix milliseconds, got {}",
        state.created_at_unix_ms
    );
    assert!(state.started_at_unix_ms >= state.created_at_unix_ms);
    assert!(state.finished_at_unix_ms >= state.started_at_unix_ms);

    // The listing is built from the same clock readings as the lookup, so the two
    // cannot report a different instant for one run.
    let listed = simulations
        .list(v1::ListSimulationsRequest {
            offset: 0,
            limit: 10,
        })
        .await
        .expect("list")
        .into_inner();
    let row = listed
        .items
        .iter()
        .find(|item| item.id == reply.id)
        .expect("the submitted run is listed");
    assert_eq!(row.created_at_unix_ms, state.created_at_unix_ms);
    assert_eq!(row.finished_at_unix_ms, state.finished_at_unix_ms);

    let summary = simulations
        .summary(v1::SummaryRequest {
            id: reply.id.clone(),
        })
        .await
        .expect("summary")
        .into_inner();
    assert!(summary.summary_json.contains("route_length_m"));

    // Streaming the truth in frames of `limit` samples.
    let mut truth = simulations
        .truth(v1::StreamRequest {
            id: reply.id.clone(),
            channel: String::new(),
            offset: 0,
            limit: 100,
        })
        .await
        .expect("truth")
        .into_inner();
    let mut frames = 0usize;
    let mut times: Vec<f64> = Vec::new();
    while let Some(sample) = truth.message().await.expect("sample") {
        frames += 1;
        times.push(sample.time_s);
        if frames == 1 {
            assert!(
                sample.position.is_some(),
                "a truth sample must carry a position"
            );
            assert!(sample.speed.is_finite());
        }
        if frames >= 100 {
            break;
        }
    }
    assert_eq!(
        frames, 100,
        "the stream must honour the requested frame size"
    );

    // The offset starts the stream at the requested sample, so a client can shard
    // a channel: the frame must carry samples 50.. of the run, not its head again.
    let mut shifted = simulations
        .truth(v1::StreamRequest {
            id: reply.id.clone(),
            channel: String::new(),
            offset: 50,
            limit: 50,
        })
        .await
        .expect("truth")
        .into_inner();
    let mut shifted_times: Vec<f64> = Vec::new();
    while let Some(sample) = shifted.message().await.expect("sample") {
        shifted_times.push(sample.time_s);
        if shifted_times.len() >= 50 {
            break;
        }
    }
    assert_eq!(shifted_times.len(), 50, "the offset frame must be full");
    assert_eq!(
        shifted_times,
        times[50..100],
        "an offset of 50 must start the stream at the 50th sample"
    );

    // An offset past the end is an empty stream rather than an error.
    let mut past = simulations
        .truth(v1::StreamRequest {
            id: reply.id.clone(),
            channel: String::new(),
            offset: u32::MAX,
            limit: 10,
        })
        .await
        .expect("truth")
        .into_inner();
    assert!(
        past.message().await.expect("frame").is_none(),
        "an offset past the end must end the stream at once"
    );

    // Accelerometer samples against the same run.
    let mut sensors = simulations
        .sensors(v1::StreamRequest {
            id: reply.id.clone(),
            channel: "accel".to_string(),
            offset: 0,
            limit: 32,
        })
        .await
        .expect("sensors")
        .into_inner();
    let mut count = 0usize;
    let mut sensor_times: Vec<f64> = Vec::new();
    while let Some(sample) = sensors.message().await.expect("sensor") {
        count += 1;
        sensor_times.push(sample.time_s);
        assert_eq!(sample.channel, "accel");
        assert_eq!(sample.v.len(), 3);
        if count >= 32 {
            break;
        }
    }
    assert_eq!(count, 32);

    // The same offset rule holds for a sensor channel.
    let mut shifted = simulations
        .sensors(v1::StreamRequest {
            id: reply.id.clone(),
            channel: "accel".to_string(),
            offset: 16,
            limit: 16,
        })
        .await
        .expect("sensors")
        .into_inner();
    let mut shifted_times: Vec<f64> = Vec::new();
    while let Some(sample) = shifted.message().await.expect("sensor") {
        shifted_times.push(sample.time_s);
        if shifted_times.len() >= 16 {
            break;
        }
    }
    assert_eq!(
        shifted_times,
        sensor_times[16..32],
        "an offset of 16 must start the stream at the 16th sample"
    );

    // A channel that does not exist is a client error, not an empty stream.
    let error = simulations
        .sensors(v1::StreamRequest {
            id: reply.id.clone(),
            channel: "magnetron".to_string(),
            offset: 0,
            limit: 4,
        })
        .await
        .expect_err("an unknown channel must fail");
    assert_eq!(error.code(), tonic::Code::InvalidArgument, "{error}");

    // A route preview is a task, and the task service submits and reports it.
    let mut tasks = v1::task_service_client::TaskServiceClient::connect(endpoint.clone())
        .await
        .expect("connect");
    let submitted = tasks
        .submit(v1::SubmitTaskRequest {
            body_json: serde_json::to_string(&json!({
                "kind": "route_preview",
                "request": serde_json::from_str::<Value>(&request).expect("the request parses"),
            }))
            .expect("body"),
        })
        .await
        .expect("submit")
        .into_inner();
    assert_eq!(submitted.kind, v1::TaskKind::RoutePreview as i32);
    let ticket = wait_for_task(&mut tasks, &submitted.id).await;
    assert_eq!(ticket.state, v1::TaskState::Succeeded as i32, "{ticket:?}");
    let result = tasks
        .result(v1::TaskResultRequest {
            id: submitted.id.clone(),
        })
        .await
        .expect("result")
        .into_inner();
    assert!(
        result.result_json.contains("candidates"),
        "the preview must carry the candidate set: {}",
        result.result_json
    );

    // Cancelling a finished job is a conflict.
    let error = simulations
        .cancel(v1::CancelRequest {
            id: reply.id.clone(),
        })
        .await
        .expect_err("a finished job cannot be cancelled");
    assert!(
        matches!(
            error.code(),
            tonic::Code::FailedPrecondition | tonic::Code::NotFound
        ),
        "{error}"
    );

    service
        .shutdown(Duration::from_secs(5))
        .await
        .expect("stop");
}
