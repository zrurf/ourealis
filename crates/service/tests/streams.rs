//! Streaming transports: Server-Sent Events, the WebSocket route and NDJSON.
//!
//! SSE and NDJSON are plain text protocols, so they are exercised over a real
//! socket here. The WebSocket protocol is covered end to end by the browser tests
//! in `web/tests/e2e`; what matters at this layer is that the route exists and
//! refuses a request that did not ask for an upgrade instead of pretending it is
//! an unknown path.

mod fixtures;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use serde_json::Value;

use ourealis::app::Service;

use fixtures::test_config;

/// Starts a service with the web page off and returns its HTTP port.
async fn start() -> (Service, u16) {
    start_with(true, 2).await
}

/// Starts a service with the web page off and returns its HTTP port.
///
/// `compression` chooses whether responses may be compressed. `max_concurrent` is the
/// number of jobs that run at once, which the cancellation test needs to be 1: with a
/// spare slot the second job would already be running, and a cancellation cannot
/// interrupt a run in progress (it takes effect at a stage boundary).
async fn start_with(compression: bool, max_concurrent: usize) -> (Service, u16) {
    let mut config = test_config();
    // A short interval means a stream that never ends shows itself while the test
    // waits rather than looking idle.
    config.http.sse_keep_alive_s = 0.5;
    config.http.compression = compression;
    config.simulation.max_concurrent = max_concurrent;
    let service = Service::start(config).await.expect("service starts");
    let port = service.http_addr().expect("bound").port();
    (service, port)
}

/// Builds a request: request line, headers, then `Connection: close` and the blank
/// line. Assembled here because an escaped CRLF inside a string literal is easy to get
/// wrong and hard to read.
fn request(version: &str, line: &str, headers: &[(&str, &str)]) -> String {
    let mut out = format!("{line} HTTP/{version}\r\n");
    for (name, value) in headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("Connection: close\r\n\r\n");
    out
}

/// A blocking HTTP request that returns the status line, headers and body.
///
/// The caller picks the version in `request`. `HTTP/1.0` matters for the streamed
/// bodies: hyper cannot know their length, so a 1.1 response is chunk-encoded and
/// the body would arrive interleaved with size lines, while a 1.0 response is
/// close-delimited and the body is exactly the bytes after the header block.
fn http(port: u16, request: &str) -> (String, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .expect("timeout");
    stream.write_all(request.as_bytes()).expect("send");
    stream.flush().expect("flush");
    let mut text = String::new();
    stream.read_to_string(&mut text).expect("read");
    match text.split_once("\r\n\r\n") {
        Some((head, body)) => (head.to_string(), body.to_string()),
        None => (text, String::new()),
    }
}

/// A blocking HTTP request that returns the response head as text.
///
/// Unlike [`http`] the body is not decoded: a compressed body is not UTF-8, and
/// only the headers are read.
fn http_head(port: u16, request: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .expect("timeout");
    stream.write_all(request.as_bytes()).expect("send");
    stream.flush().expect("flush");
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).expect("read");
    let text = String::from_utf8_lossy(&bytes);
    match text.split_once("\r\n\r\n") {
        Some((head, _)) => head.to_string(),
        None => text.to_string(),
    }
}

/// Opens the event stream of one job and returns its response head with a reader
/// positioned after the header block.
///
/// The request is `HTTP/1.0`, so the body is close-delimited: a 1.1 stream is
/// chunk-encoded, which would interleave chunk sizes with the frames and leave the
/// terminating chunk behind as a last line.
fn sse_stream(port: u16, id: &str) -> (String, BufReader<TcpStream>) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .expect("timeout");
    stream
        .write_all(
            format!(
                "GET /api/v1/simulations/{id}/events HTTP/1.0\r\nHost: 127.0.0.1\r\n\
                 Accept: text/event-stream\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .expect("send");

    let mut reader = BufReader::new(stream);
    let mut head = String::new();
    reader.read_line(&mut head).expect("status line");
    assert!(
        head.starts_with("HTTP/1.0 200"),
        "unexpected status: {head}"
    );
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("header");
        head.push_str(&line);
        if line == "\r\n" || line.is_empty() {
            break;
        }
    }
    (head, reader)
}

/// Reads SSE frames until a terminal event arrives or the deadline passes.
///
/// Returns the `(event, data)` pairs seen. An event name carries over to the
/// data line that follows it, as the protocol specifies.
fn read_frames(reader: &mut impl BufRead, deadline: Instant) -> Vec<(String, String)> {
    let mut frames: Vec<(String, String)> = Vec::new();
    let mut current = String::new();
    while Instant::now() < deadline {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) => panic!("reading the event stream failed: {error}"),
        }
        let line = line.trim_end().to_string();
        if let Some(name) = line.strip_prefix("event: ") {
            current = name.to_string();
        } else if let Some(data) = line.strip_prefix("data: ") {
            frames.push((current.clone(), data.to_string()));
            if current == "done" || current == "error" {
                break;
            }
        } else if let Some(comment) = line.strip_prefix(':') {
            // A comment is the keep-alive frame; it must not be mistaken for data.
            assert_eq!(
                comment.trim(),
                "keep-alive",
                "unexpected keep-alive frame: {line:?}"
            );
        }
    }
    frames
}

/// Reads the rest of a closed stream, asserting that the server ended it instead
/// of holding the connection open.
///
/// A read that blocks until the shortened timeout means the stream is still open,
/// which is the failure this checks for.
fn assert_closed(reader: &mut BufReader<TcpStream>) {
    reader
        .get_mut()
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("timeout");
    loop {
        let mut tail = String::new();
        match reader.read_line(&mut tail) {
            Ok(0) => return,
            // The blank line that ends the terminal frame is not a new frame.
            Ok(_) if tail.trim().is_empty() => {}
            Ok(_) => panic!("the stream kept talking after the terminal event: {tail:?}"),
            Err(error) => panic!("the stream stayed open after the terminal event: {error}"),
        }
    }
}

/// Uploads the synthetic map through the store of a running service.
async fn upload(service: &Service) -> String {
    let image = fixtures::map_image();
    let id = service.state().maps.next_id("streams");
    let summary =
        ourealis::api::maps::summarise(&id, &image, "import", ourealis::store::created_at_ms())
            .expect("summary");
    service
        .state()
        .maps
        .insert(ourealis::store::MapEntry {
            id: id.clone(),
            name: summary.name.clone(),
            source: "import".to_string(),
            created_at_ms: ourealis::store::created_at_ms(),
            summary,
            bytes: std::sync::Arc::new(image),
        })
        .expect("insert");
    id
}

/// A submit body for the compact map.
fn submit_body(map_id: &str) -> String {
    serde_json::json!({
        "name": "streams",
        "map": { "kind": "id", "id": map_id },
        "route": {
            "mode": "standard",
            "start": { "x": 40.0, "y": 60.0 },
            "goal": { "x": 250.0, "y": 150.0 }
        },
        "person": { "preset": "moderate" },
        "seed": 31337,
        "settings": { "with_metrics": false }
    })
    .to_string()
}

/// Submits a job and returns its id.
fn submit(port: u16, map_id: &str) -> String {
    let body = submit_body(map_id);
    let request = format!(
        "POST /api/v1/simulations HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (head, body) = http(port, &request);
    assert!(
        head.starts_with("HTTP/1.1 202"),
        "submission failed: {head}\n{body}"
    );
    let reply: Value = serde_json::from_str(&body).expect("reply");
    reply["id"].as_str().expect("an id").to_string()
}

#[test]
fn the_event_stream_reports_progress_and_finishes_with_done() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let (service, port) = runtime.block_on(start());
    let map_id = runtime.block_on(upload(&service));
    let id = submit(port, &map_id);

    // SSE is a plain text protocol, so the stream is read line by line until the
    // terminal event arrives; the job was submitted a moment ago, so the subscriber
    // is watching it from before it starts.
    let (head, mut reader) = sse_stream(port, &id);
    assert!(
        head.to_ascii_lowercase()
            .contains("content-type: text/event-stream"),
        "the stream must be announced as SSE: {head}"
    );

    let events = read_frames(&mut reader, Instant::now() + Duration::from_secs(180));

    let names: Vec<&str> = events.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names.first().copied(),
        Some("state"),
        "a subscriber must be told the current state first: {names:?}"
    );
    assert!(
        names.contains(&"done"),
        "the stream must end with a done event, got {names:?}"
    );
    let done = events
        .iter()
        .find(|(name, _)| name == "done")
        .map(|(_, data)| data)
        .expect("done payload");
    let payload: Value = serde_json::from_str(done).expect("done payload is JSON");
    assert_eq!(payload["event"]["state"], "succeeded");
    assert!(
        payload["event"]["summary_url"]
            .as_str()
            .unwrap_or_default()
            .contains(&id)
    );

    // The state event before it must have carried a state and a stage.
    let state = events
        .iter()
        .find(|(name, _)| name == "state")
        .map(|(_, data)| data)
        .expect("state payload");
    let payload: Value = serde_json::from_str(state).expect("state payload is JSON");
    assert!(payload["event"]["state"].is_string());
    assert!(payload["event"]["stage"].is_string());

    // The stream must end after the terminal event instead of holding the
    // connection open: a client that waits for the close must not need a timeout
    // to stop reading.
    assert_closed(&mut reader);

    runtime
        .block_on(service.shutdown(Duration::from_secs(5)))
        .expect("stop");
}

#[test]
fn the_event_stream_of_a_finished_job_reports_the_outcome_at_once() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let (service, port) = runtime.block_on(start());
    let map_id = runtime.block_on(upload(&service));
    let id = submit(port, &map_id);

    // Wait for the job to finish before subscribing, which is where a client that
    // lost the live stream and reconnects finds itself.
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let (_, body) = http(
            port,
            &format!(
                "GET /api/v1/simulations/{id} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
            ),
        );
        let state: Value = serde_json::from_str(&body).expect("state");
        if state["state"] == "succeeded" {
            break;
        }
        assert!(Instant::now() < deadline, "job did not finish: {state}");
        std::thread::sleep(Duration::from_millis(100));
    }

    // Read the whole stream: the outcome must arrive without waiting for a change
    // that will never happen, and the connection must then close.
    let (_, mut reader) = sse_stream(port, &id);
    reader
        .get_mut()
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("timeout");
    let mut names: Vec<String> = Vec::new();
    let mut closed = false;
    let mut current = String::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                closed = true;
                break;
            }
            Ok(_) => {}
            Err(error) => panic!("reading the event stream failed: {error}"),
        }
        let line = line.trim_end();
        if let Some(name) = line.strip_prefix("event: ") {
            current = name.to_string();
        } else if let Some(data) = line.strip_prefix("data: ") {
            let payload: Value = serde_json::from_str(data).expect("event payload is JSON");
            assert!(payload["event"]["state"].is_string());
            names.push(current.clone());
        }
    }

    assert!(closed, "the stream of a finished job must close");
    assert_eq!(
        names.first().map(String::as_str),
        Some("done"),
        "a finished job must report its outcome immediately: {names:?}"
    );
    assert!(
        names.iter().all(|name| name == "done"),
        "only the outcome may be sent for a job that already finished: {names:?}"
    );

    runtime
        .block_on(service.shutdown(Duration::from_secs(5)))
        .expect("stop");
}

#[test]
fn the_compression_layer_follows_the_configuration() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let request = "GET /api/v1/presets HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept-Encoding: gzip\r\n\
                   Connection: close\r\n\r\n";

    // `http.compression` defaults to true, so a client that offers gzip gets it.
    let (service, port) = runtime.block_on(start_with(true, 2));
    let head = http_head(port, request);
    assert!(
        head.to_ascii_lowercase().contains("content-encoding: gzip"),
        "a client that offers gzip must get a compressed response: {head}"
    );
    runtime
        .block_on(service.shutdown(Duration::from_secs(5)))
        .expect("stop");

    // Turning it off must be visible: the same request comes back uncompressed.
    let (service, port) = runtime.block_on(start_with(false, 2));
    let head = http_head(port, request);
    assert!(
        !head.to_ascii_lowercase().contains("content-encoding"),
        "http.compression = false must leave the response uncompressed: {head}"
    );
    runtime
        .block_on(service.shutdown(Duration::from_secs(5)))
        .expect("stop");
}

#[test]
fn a_cancelled_job_ends_its_event_stream() {
    // A terminal state has to be accompanied by a terminal event, or every watcher of
    // that job stays connected forever: cancellation is the case that used to publish
    // only a state snapshot.
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let (service, port) = runtime.block_on(start_with(true, 1));
    let map_id = runtime.block_on(upload(&service));

    // One worker slot, two submissions: the second waits, so it can be cancelled
    // before it ever runs.
    let first = submit(port, &map_id);
    let second = submit(port, &map_id);
    let (status, _) = http(
        port,
        &request("1.1", &format!("DELETE /api/v1/simulations/{second}"), &[]),
    );
    assert!(
        status.starts_with("HTTP/1.1 204") || status.starts_with("HTTP/1.1 409"),
        "cancelling the queued job: {status}"
    );

    // Reading the stream afterwards must still end at once, with a terminal event.
    let (_, body) = http(
        port,
        &request(
            "1.0",
            &format!("GET /api/v1/simulations/{second}/events"),
            &[("Accept", "text/event-stream")],
        ),
    );
    assert!(
        body.contains("event: done") || body.contains("event: error"),
        "the stream must carry a terminal event, got: {body}"
    );
    assert!(
        body.contains("cancel"),
        "and it must say the job was cancelled: {body}"
    );

    // The first job is unaffected and still finishes.
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let (_, state) = http(
            port,
            &request("1.0", &format!("GET /api/v1/simulations/{first}"), &[]),
        );
        let parsed: Value = serde_json::from_str(&state).expect("state");
        if parsed["state"] == "succeeded" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the first job did not finish: {parsed}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }

    runtime
        .block_on(service.shutdown(Duration::from_secs(5)))
        .expect("stop");
}

#[test]
fn a_request_without_upgrade_headers_is_refused_rather_than_unknown() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let (service, port) = runtime.block_on(start());
    let (head, _) = http(
        port,
        "GET /api/v1/simulations/whatever/ws HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    // The route exists: a plain GET is a bad upgrade request (400), not a 404 for
    // an unknown path.
    assert!(
        head.starts_with("HTTP/1.1 400") || head.starts_with("HTTP/1.1 426"),
        "expected a protocol error from the WebSocket route, got {head}"
    );
    runtime
        .block_on(service.shutdown(Duration::from_secs(5)))
        .expect("stop");
}

#[test]
fn the_ndjson_stream_yields_one_json_object_per_line() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let (service, port) = runtime.block_on(start());
    let map_id = runtime.block_on(upload(&service));
    let id = submit(port, &map_id);

    // Wait for the job before reading its stream.
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let (_, body) = http(
            port,
            &format!(
                "GET /api/v1/simulations/{id} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
            ),
        );
        let state: Value = serde_json::from_str(&body).expect("state");
        if state["state"] == "succeeded" {
            break;
        }
        assert!(Instant::now() < deadline, "job did not finish: {state}");
        std::thread::sleep(Duration::from_millis(100));
    }

    let (head, body) = http(
        port,
        &format!(
            "GET /api/v1/simulations/{id}/truth.ndjson HTTP/1.0\r\nHost: 127.0.0.1\r\n\
             Connection: close\r\n\r\n"
        ),
    );
    // The response echoes the request's version, so only the status is asserted.
    assert!(head.contains(" 200 "), "{head}");
    let mut lines = body.lines().filter(|line| !line.trim().is_empty());
    let mut count = 0usize;
    for line in lines.by_ref() {
        let sample: Value = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("line is not JSON ({error}): {line}"));
        assert!(sample["time_s"].is_number() || sample["item"]["time_s"].is_number());
        count += 1;
        if count >= 20 {
            break;
        }
    }
    assert!(
        count >= 20,
        "expected at least 20 streamed samples, got {count}"
    );
    runtime
        .block_on(service.shutdown(Duration::from_secs(5)))
        .expect("stop");
}
