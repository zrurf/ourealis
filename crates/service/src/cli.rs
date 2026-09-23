//! Command line interface.
//!
//! The service takes almost all of its behaviour from the configuration file; the
//! command line only says *which* file to read, how loudly to log, and offers a
//! way to print the effective configuration without starting anything.

use std::path::PathBuf;

use clap::Parser;

/// Ourealis service: gRPC, HTTP, WebSocket and SSE facades over the simulator.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "ourealis",
    version,
    about = "Ourealis service: simulation facades and web interface",
    long_about = None,
)]
pub struct Cli {
    /// Configuration file to read.
    ///
    /// Defaults to `config.toml` next to the executable, or `OUREALIS_CONFIG`
    /// when that is set. A path given here must exist.
    #[arg(short, long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Print the effective configuration, then exit.
    ///
    /// Useful to see which defaults apply and which keys a file actually sets.
    #[arg(long)]
    pub print_config: bool,

    /// Log level, overriding `log.level` in the configuration file.
    ///
    /// Accepts a single level (`debug`) or an `env_logger`-style directive list
    /// (`info,ourealis::task=trace`).
    #[arg(long, value_name = "LEVEL")]
    pub log: Option<String>,
}

impl Cli {
    /// Log level requested on the command line, if any.
    pub fn log_level(&self) -> Option<&str> {
        self.log.as_deref()
    }
}
