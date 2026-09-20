//! Shared fixtures of the service test suites.
//!
//! Each test binary compiles its own copy of this module, so a helper used by one
//! suite is unused code in the others; the allowance is what keeps that from
//! reading as a defect in every binary but one.
//!
//! Everything here is built in memory: a small synthetic map, a configuration with
//! the facades on ephemeral ports, and an `AppState` that uses the in-memory store.
//! The map is generated with the `compact` preset, which is the smallest shape the
//! simulator tests use and keeps a full run at a second or two in a debug build.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use ourealis::app::AppState;
use ourealis::config::{Config, LoadedConfig};
use ourealis_map_format::Map;
use ourealis_map_format::synthetic::{self, SyntheticMapSpec};

/// A synthetic map image, built once per test process.
pub fn map_image() -> Vec<u8> {
    synthetic::build(&SyntheticMapSpec::compact()).expect("synthetic map builds")
}

/// The map image parsed, for tests that need its geometry.
pub fn map() -> Map {
    Map::from_bytes(map_image()).expect("synthetic map opens")
}

/// A configuration with both facades on ephemeral ports and no web page.
///
/// Tests that bind real sockets take port 0 from the system rather than a fixed
/// number, so two test binaries never collide.
pub fn test_config() -> Config {
    let mut config = Config::default();
    config.server.http_listen = "127.0.0.1:0".to_string();
    config.server.rpc_listen = "127.0.0.1:0".to_string();
    config.server.web_enabled = false;
    config.server.shutdown_grace_s = 2.0;
    config.simulation.max_concurrent = 2;
    config.simulation.queue_capacity = 4;
    config.log.level = "warn".to_string();
    config
}

/// An `AppState` over the test configuration.
pub fn test_state() -> Arc<AppState> {
    AppState::new(test_config()).expect("state builds")
}

/// A configuration loaded from TOML text.
pub fn loaded(text: &str) -> LoadedConfig {
    Config::from_toml(text).expect("configuration parses")
}

/// Standard base64 with padding, as the service's decoder accepts.
///
/// Written out rather than pulled in: the tests need an encoder for one request
/// field, and a dependency for that would be larger than the function.
pub fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let triple = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

/// A unique temporary directory, removed by the caller.
pub fn temp_dir(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "ourealis-service-{tag}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&path).expect("temp dir");
    path
}
