//! Configuration loading: defaults, partial files, version handling and rejection.

mod fixtures;

use ourealis::config::{CONFIG_VERSION, Config, LogFormat, StorageMode};
use ourealis::error::ErrorKind;

use fixtures::loaded;

#[test]
fn an_empty_file_is_a_valid_configuration() {
    let loaded = Config::from_toml("").expect("empty configuration");
    assert_eq!(loaded.config.config_version, CONFIG_VERSION);
    assert_eq!(loaded.config, Config::default());
    assert!(loaded.unknown_keys.is_empty());
}

#[test]
fn defaults_are_loopback_and_serve_every_facade() {
    let config = Config::default();
    assert!(config.server.rpc_enabled && config.server.http_enabled && config.server.web_enabled);
    // A default configuration must not be reachable from the network.
    assert!(config.server.http_listen.starts_with("127.0.0.1:"));
    assert!(config.server.rpc_listen.starts_with("127.0.0.1:"));
    assert_eq!(config.storage.mode, StorageMode::Memory);
    assert_eq!(config.log.format, LogFormat::Pretty);
}

#[test]
fn a_partial_file_keeps_the_defaults_of_the_fields_it_omits() {
    let loaded = loaded(
        r#"
        [server]
        http_listen = "0.0.0.0:9999"

        [http]
        max_body_mb = 8
        "#,
    );
    assert_eq!(loaded.config.server.http_listen, "0.0.0.0:9999");
    assert_eq!(loaded.config.http.max_body_mb, 8);
    // Untouched sections keep their defaults.
    assert_eq!(
        loaded.config.server.rpc_listen,
        Config::default().server.rpc_listen
    );
    assert_eq!(
        loaded.config.simulation.max_concurrent,
        Config::default().simulation.max_concurrent
    );
    assert_eq!(loaded.config.http.max_page_size, 100_000);
}

#[test]
fn unknown_keys_are_reported_and_ignored() {
    let loaded = loaded(
        r#"
        [server]
        http_listen = "127.0.0.1:0"
        listen = "typo"

        [future_section]
        answer = 42
        "#,
    );
    assert_eq!(
        loaded.unknown_keys,
        vec!["future_section".to_string(), "server.listen".to_string()]
    );
    assert_eq!(loaded.config.server.http_listen, "127.0.0.1:0");
}

#[test]
fn a_newer_version_is_refused() {
    let error = Config::from_toml(&format!("config_version = {}", CONFIG_VERSION + 1))
        .expect_err("a newer version must be refused");
    assert_eq!(error.kind(), ErrorKind::Invalid);
    assert!(
        error.to_string().contains("newer than this build supports"),
        "unexpected message: {error}"
    );
}

#[test]
fn a_version_below_the_current_one_is_migrated() {
    // Version 1 is the only layout, so the note list stays empty; the chain and
    // its reporting exist for the next bump.
    let loaded = loaded(&format!("config_version = {CONFIG_VERSION}"));
    assert!(loaded.migrations.is_empty());

    let error = Config::from_toml("config_version = 0").expect_err("version 0 is not a version");
    assert!(error.to_string().contains("at least 1"), "got {error}");
    let error = Config::from_toml("config_version = \"one\"").expect_err("not an integer");
    assert!(
        error.to_string().contains("must be an integer"),
        "got {error}"
    );
}

#[test]
fn the_web_page_requires_the_http_facade() {
    let error = Config::from_toml(
        r#"
        [server]
        http_enabled = false
        web_enabled = true
        "#,
    )
    .expect_err("web without http cannot run");
    assert_eq!(error.kind(), ErrorKind::Invalid);
    assert!(
        error
            .to_string()
            .contains("web.enabled requires http.enabled"),
        "unexpected message: {error}"
    );
}

#[test]
fn turning_every_facade_off_is_refused() {
    let error = Config::from_toml(
        r#"
        [server]
        rpc_enabled = false
        http_enabled = false
        web_enabled = false
        "#,
    )
    .expect_err("a service with no facade serves nothing");
    assert!(
        error.to_string().contains("nothing would be served"),
        "got {error}"
    );

    // The web page alone is fine once HTTP is on.
    let loaded = loaded(
        r#"
        [server]
        rpc_enabled = false
        http_enabled = true
        web_enabled = true
        "#,
    );
    assert!(!loaded.config.server.rpc_enabled);
    assert!(!loaded.config.server.rpc_listen.is_empty());
}

#[test]
fn unusable_values_are_rejected_with_the_field_name() {
    let cases = [
        ("[log]\nlevel = \"loud\"", "log.level"),
        (
            "[server]\nhttp_listen = \"127.0.0.1\"",
            "server.http_listen",
        ),
        (
            "[server]\nhttp_listen = \"127.0.0.1:port\"",
            "server.http_listen",
        ),
        ("[server]\nrpc_listen = \":0\"", "server.rpc_listen"),
        ("[http]\nmax_body_mb = 0", "http.max_body_mb"),
        ("[http]\nmax_page_size = 0", "http.max_page_size"),
        (
            "[http]\ncors_allow_origins = [\"localhost:5173\"]",
            "http.cors_allow_origins",
        ),
        (
            "[simulation]\nmax_concurrent = 0",
            "simulation.max_concurrent",
        ),
        (
            "[simulation]\nqueue_capacity = 0",
            "simulation.queue_capacity",
        ),
        (
            "[simulation]\nstream_frame_samples = 0",
            "simulation.stream_frame_samples",
        ),
        ("[storage]\nkeep_maps = 0", "storage.keep_maps"),
        (
            "[server]\nshutdown_grace_s = 1000.0",
            "server.shutdown_grace_s",
        ),
    ];
    for (text, field) in cases {
        let error = Config::from_toml(text).expect_err("value must be rejected");
        assert_eq!(error.kind(), ErrorKind::Invalid, "for {text}");
        assert!(
            error.to_string().contains(field),
            "the message for {text:?} should name {field}: {error}"
        );
    }
}

#[test]
fn a_log_directive_list_is_accepted() {
    let loaded = loaded("[log]\nlevel = \"info,ourealis::job=trace\"");
    assert_eq!(loaded.config.log.level, "info,ourealis::job=trace");
    // The first directive decides whether the level is usable at all.
    let error = Config::from_toml("[log]\nlevel = \"verbose,ourealis=trace\"")
        .expect_err("an unknown base level is refused");
    assert!(error.to_string().contains("log.level"), "got {error}");
}

#[test]
fn the_effective_configuration_renders_as_toml_that_loads_back() {
    let original = loaded(
        r#"
        [server]
        http_listen = "0.0.0.0:8123"
        [simulation]
        max_concurrent = 5
        "#,
    );
    let text = original.config.to_toml();
    let again = loaded(&text);
    assert_eq!(again.config, original.config);
    assert!(
        text.contains("http_listen = \"0.0.0.0:8123\""),
        "got {text}"
    );
    assert!(text.contains("config_version = 1"), "got {text}");
}

#[test]
fn a_missing_configuration_file_is_an_error_only_when_named() {
    let missing = std::path::Path::new("this-file-does-not-exist.toml");
    let error = Config::load(Some(missing)).expect_err("an explicit path must exist");
    assert!(
        error.to_string().contains("does not exist"),
        "unexpected message: {error}"
    );
}
