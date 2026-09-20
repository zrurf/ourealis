//! Service entry point.
//!
//! Responsibilities, in order: pick the allocator, read the command line, load and
//! report the configuration, initialise logging, start the runtime and hand over to
//! [`app::run`]. Anything that has to happen before the runtime exists lives here;
//! anything that needs the runtime lives in `app`.

use std::process::ExitCode;

use clap::Parser;
use mimalloc::MiMalloc;

use ourealis::cli::Cli;
use ourealis::config::{Config, LoadedConfig};
use ourealis::{app, config};

#[global_allocator]
static ALLOCATOR: MiMalloc = MiMalloc;

fn main() -> ExitCode {
    let cli = Cli::parse();

    let loaded = match Config::load(cli.config.as_deref()) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };

    init_logging(&cli, &loaded);
    report_config(&loaded);

    if cli.print_config {
        print!("{}", loaded.config.to_toml());
        return ExitCode::SUCCESS;
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("ourealis")
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!("cannot start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(app::run(loaded.config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("service stopped: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Installs the global tracing subscriber.
///
/// The level comes from the command line, then `RUST_LOG`, then the configuration
/// file. Colour is turned off when the output is not a terminal, because a
/// redirected log file full of escape sequences is unreadable.
fn init_logging(cli: &Cli, loaded: &LoadedConfig) {
    use tracing_subscriber::EnvFilter;

    let configured = cli
        .log_level()
        .map(str::to_string)
        .unwrap_or_else(|| loaded.config.log.level.clone());
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&configured))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let ansi = loaded.config.log.ansi && std::io::IsTerminal::is_terminal(&std::io::stdout());
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(ansi)
        .with_target(true);
    let result = match loaded.config.log.format {
        config::LogFormat::Pretty => builder.try_init(),
        config::LogFormat::Compact => builder.compact().try_init(),
        config::LogFormat::Json => builder.json().try_init(),
    };
    if let Err(error) = result {
        eprintln!("warning: logging was already initialised: {error}");
    }
}

/// Logs where the configuration came from and what did not apply.
fn report_config(loaded: &LoadedConfig) {
    match &loaded.source {
        Some(path) => tracing::info!("configuration: {}", path.display()),
        None => tracing::info!("configuration: defaults (no config.toml found)"),
    }
    for note in &loaded.migrations {
        tracing::warn!("configuration: {note}");
    }
    if !loaded.unknown_keys.is_empty() {
        tracing::warn!(
            "configuration: {} key(s) are not understood by this build and were ignored: {}",
            loaded.unknown_keys.len(),
            loaded.unknown_keys.join(", ")
        );
    }
}
