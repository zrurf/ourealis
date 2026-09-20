//! Configuration: defaults, file loading, validation and version handling.
//!
//! Every field has a default, so an empty file — or no file at all — is a valid
//! configuration. The file declares `config_version`; a file written for a newer
//! major of this service is refused rather than interpreted approximately, and a
//! file written for an older one is migrated by [`migrations`].
//!
//! Loading order: `--config <path>`, then `OUREALIS_CONFIG`, then `config.toml`
//! next to the executable, then the defaults alone.

pub mod defaults;
pub mod schema;

use std::path::{Path, PathBuf};

use crate::error::{Result, ServiceError};

pub use schema::{
    Config, HttpConfig, LogConfig, LogFormat, RpcConfig, ServerConfig, SimulationConfigSection,
    StorageConfig, StorageMode, WebConfig,
};

/// Configuration file layout version this build understands.
pub const CONFIG_VERSION: u32 = 1;

/// Environment variable naming a configuration file.
pub const CONFIG_ENV: &str = "OUREALIS_CONFIG";

/// What a load produced, for logging and for `--print-config`.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    /// The effective configuration.
    pub config: Config,
    /// File the configuration came from, when one was found.
    pub source: Option<PathBuf>,
    /// Keys the file set that this build does not know, in dotted form.
    pub unknown_keys: Vec<String>,
    /// Version notes, oldest first.
    pub migrations: Vec<String>,
}

impl LoadedConfig {
    /// Effective configuration without a file.
    pub fn from_defaults() -> Self {
        Self {
            config: Config::default(),
            source: None,
            unknown_keys: Vec::new(),
            migrations: Vec::new(),
        }
    }
}

impl Config {
    /// Loads the configuration for a run.
    ///
    /// `explicit` is the `--config` argument. A path given explicitly but not
    /// present is an error; the implicit locations are optional.
    pub fn load(explicit: Option<&Path>) -> Result<LoadedConfig> {
        if let Some(path) = explicit {
            if !path.is_file() {
                return Err(ServiceError::Config(format!(
                    "configuration file {} does not exist",
                    path.display()
                )));
            }
            return Self::from_file(path);
        }
        if let Some(path) = std::env::var_os(CONFIG_ENV) {
            let path = PathBuf::from(path);
            if !path.is_file() {
                return Err(ServiceError::Config(format!(
                    "{CONFIG_ENV} points at {}, which does not exist",
                    path.display()
                )));
            }
            return Self::from_file(&path);
        }
        if let Some(path) = default_path()
            && path.is_file()
        {
            return Self::from_file(&path);
        }
        Ok(LoadedConfig::from_defaults())
    }

    /// Loads a configuration file.
    pub fn from_file(path: &Path) -> Result<LoadedConfig> {
        let text = std::fs::read_to_string(path).map_err(|error| {
            ServiceError::Config(format!("cannot read {}: {error}", path.display()))
        })?;
        let mut loaded = Self::from_toml(&text)?;
        loaded.source = Some(path.to_path_buf());
        Ok(loaded)
    }

    /// Parses a configuration from TOML text.
    pub fn from_toml(text: &str) -> Result<LoadedConfig> {
        let raw: toml::Value = toml::from_str(text).map_err(|error| {
            ServiceError::Config(format!("configuration is not valid TOML: {error}"))
        })?;
        let version = match raw.get("config_version") {
            None => CONFIG_VERSION,
            Some(value) => {
                let value = value.as_integer().ok_or_else(|| {
                    ServiceError::Config("config_version must be an integer".to_string())
                })?;
                // Version 0 names no layout, so it is refused like a negative
                // one rather than migrated from nowhere.
                u32::try_from(value)
                    .ok()
                    .filter(|version| *version >= 1)
                    .ok_or_else(|| {
                        ServiceError::Config(format!(
                            "config_version must be at least 1, got {value}"
                        ))
                    })?
            }
        };
        let migrations = migrations::plan(version)?;

        let config: Config = toml::from_str(text).map_err(|error| {
            ServiceError::Config(format!("configuration is not valid: {error}"))
        })?;
        let unknown_keys = unknown_keys(&raw, &config);
        config.validate()?;
        Ok(LoadedConfig {
            config,
            source: None,
            unknown_keys,
            migrations,
        })
    }

    /// Renders the effective configuration as TOML.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_else(|error| format!("# cannot render: {error}\n"))
    }

    /// Rejects combinations that cannot run.
    pub fn validate(&self) -> Result<()> {
        if self.server.web_enabled && !self.server.http_enabled {
            return Err(ServiceError::Config(
                "web.enabled requires http.enabled: the page is served over the HTTP facade"
                    .to_string(),
            ));
        }
        if !self.server.http_enabled && !self.server.rpc_enabled {
            return Err(ServiceError::Config(
                "both rpc.enabled and http.enabled are false; nothing would be served".to_string(),
            ));
        }
        self.log.validate()?;
        self.server.validate()?;
        self.http.validate()?;
        self.simulation.validate()?;
        self.storage.validate()
    }
}

/// Path of the configuration file next to the running executable.
pub fn default_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    Some(dir.join("config.toml"))
}

/// Unknown-key detection.
///
/// Unknown keys are reported rather than rejected: a file written for a later
/// minor of this service must still start, and the operator has to be able to see
/// which parts of it did nothing. The comparison walks the file against the
/// serialised effective configuration, so a section that was misspelled is
/// reported as a whole.
fn unknown_keys(raw: &toml::Value, effective: &Config) -> Vec<String> {
    let Ok(known) = serde_json::to_value(effective) else {
        return Vec::new();
    };
    let mut unknown = Vec::new();
    collect(raw, &known, "", &mut unknown);
    unknown.sort();
    unknown
}

fn collect(raw: &toml::Value, known: &serde_json::Value, prefix: &str, out: &mut Vec<String>) {
    let (toml::Value::Table(table), serde_json::Value::Object(known)) = (raw, known) else {
        return;
    };
    for (key, value) in table {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match known.get(key) {
            Some(expected) => collect(value, expected, &path, out),
            None => out.push(path),
        }
    }
}

/// Version compatibility and the migration chain.
pub mod migrations {
    use crate::config::CONFIG_VERSION;
    use crate::error::{Result, ServiceError};

    /// Checks a file's declared version and lists the migrations to apply.
    ///
    /// The chain is empty while version 1 is the only layout, but the call site
    /// and the log lines exist so a future version bump has one place to live.
    pub fn plan(version: u32) -> Result<Vec<String>> {
        if version > CONFIG_VERSION {
            return Err(ServiceError::Config(format!(
                "configuration version {version} is newer than this build supports \
                 ({CONFIG_VERSION}); upgrade the service or rewrite the file"
            )));
        }
        let mut applied = Vec::new();
        for from in version..CONFIG_VERSION {
            applied.push(format!(
                "migrated configuration version {from} to {}",
                from + 1
            ));
        }
        Ok(applied)
    }
}
