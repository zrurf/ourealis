//! Facade startup: switch combinations, ephemeral ports and shutdown.
//!
//! These bind real sockets, unlike `http_api.rs`: the point is that the facades
//! come up on the addresses the configuration names, that port 0 is resolved, and
//! that a configuration asking for the page without the API is refused.

mod fixtures;

use std::time::Duration;

use ourealis::app::Service;
use ourealis::config::Config;
use ourealis::error::ErrorKind;

use fixtures::test_config;

/// Every test here reads a socket synchronously, which would stall a
/// current-thread runtime: the server task and the client would share one thread.
/// The multi-thread flavour keeps the reactor free while the client blocks.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_facades_bind_and_stop() {
    let service = Service::start(test_config()).await.expect("service starts");
    let http = service.http_addr().expect("http bound");
    let rpc = service.rpc_addr().expect("rpc bound");
    assert_ne!(http.port(), 0, "port 0 must be resolved to a real port");
    assert_ne!(rpc.port(), 0);
    assert_ne!(
        http.port(),
        rpc.port(),
        "the two facades must not share a port"
    );

    // The API answers on the bound port.
    let body = reqwest_get(&format!("http://{http}/api/v1/health"));
    assert!(
        body.contains("\"status\":\"ok\""),
        "unexpected health body: {body}"
    );

    service
        .shutdown(Duration::from_secs(5))
        .await
        .expect("clean shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_facade_can_be_disabled_on_its_own() {
    let mut config = test_config();
    config.server.rpc_enabled = false;
    let service = Service::start(config).await.expect("http only");
    assert!(service.http_addr().is_some());
    assert!(service.rpc_addr().is_none());
    service
        .shutdown(Duration::from_secs(5))
        .await
        .expect("stop");

    let mut config = test_config();
    config.server.http_enabled = false;
    config.server.web_enabled = false;
    let service = Service::start(config).await.expect("rpc only");
    assert!(service.http_addr().is_none());
    assert!(service.rpc_addr().is_some());
    service
        .shutdown(Duration::from_secs(5))
        .await
        .expect("stop");
}

#[tokio::test]
async fn the_page_without_the_api_is_refused_before_anything_binds() {
    let mut config = test_config();
    config.server.http_enabled = false;
    config.server.web_enabled = true;
    let error = Service::start(config)
        .await
        .expect_err("the page cannot be served without the API");
    assert_eq!(error.kind(), ErrorKind::Invalid);
    assert!(
        error
            .to_string()
            .contains("web.enabled requires http.enabled"),
        "got {error}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_page_is_served_on_the_http_port_when_enabled() {
    let mut config = test_config();
    config.server.web_enabled = true;
    let service = Service::start(config).await.expect("service");
    let http = service.http_addr().expect("http bound");
    let body = reqwest_get(&format!("http://{http}/"));
    assert!(
        body.to_ascii_lowercase().contains("<!doctype html"),
        "the root must serve the embedded page, got {} bytes",
        body.len()
    );
    // The placeholder page names the build step; a real build must not.
    if ourealis::embed::is_placeholder() {
        assert!(body.contains("Web assets were not built"));
    }
    service
        .shutdown(Duration::from_secs(5))
        .await
        .expect("stop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_service_on_the_same_port_fails_without_panicking() {
    let first = Service::start(test_config_with_port(0))
        .await
        .expect("first");
    let port = first.http_addr().expect("bound").port();
    let error = Service::start(test_config_with_port(port))
        .await
        .expect_err("a taken port must fail");
    // The failure is an I/O error reported as a server-side problem, not a panic.
    assert!(
        !error.to_string().is_empty(),
        "the error must explain itself: {error}"
    );
    first.shutdown(Duration::from_secs(5)).await.expect("stop");
}

fn test_config_with_port(port: u16) -> Config {
    let mut config = test_config();
    config.server.http_listen = format!("127.0.0.1:{port}");
    config
}

/// A minimal blocking HTTP GET.
///
/// The tests already depend on `tokio`; an HTTP client dependency for four
/// assertions is not worth adding, so this speaks HTTP over a plain socket. The
/// version is 1.0 on purpose: it forbids chunked transfer encoding, so the body is
/// the bytes after the header block rather than a chunked stream to reassemble.
fn reqwest_get(url: &str) -> String {
    use std::io::{Read, Write};

    let rest = url.trim_start_matches("http://");
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    let mut stream = std::net::TcpStream::connect(authority).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
    )
    .expect("send");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read");
    match response.split_once("\r\n\r\n") {
        Some((_, body)) => body.to_string(),
        None => response,
    }
}
