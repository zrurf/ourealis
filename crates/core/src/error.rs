//! Error type of the simulator.

use ourealis_map_format::MapError;

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, CoreError>;

/// Everything that can go wrong while planning or simulating a run.
///
/// Variant fields name the entity involved so the message points straight at
/// the map layer, waypoint or configuration entry that caused the failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
#[allow(missing_docs)]
pub enum CoreError {
    /// Failure inside the map container.
    #[error("map error: {0}")]
    Map(#[from] MapError),

    /// The requested position lies outside the map bounds or is impassable.
    #[error("position ({x:.1}, {y:.1}) is not usable: {reason}")]
    UnusablePosition { x: f64, y: f64, reason: String },

    /// No path exists between two points under the current constraints.
    #[error("no path from ({from_x:.1}, {from_y:.1}) to ({to_x:.1}, {to_y:.1})")]
    NoPath {
        from_x: f64,
        from_y: f64,
        to_x: f64,
        to_y: f64,
    },

    /// A requested layer or feature dimension is missing from the map.
    #[error("map is missing required {what}")]
    MissingLayer { what: &'static str },

    /// Weight vectors and feature dimensions disagree.
    #[error("weight dimension {weights} does not match feature dimension {features}")]
    DimensionMismatch { weights: usize, features: usize },

    /// The fixed-point iteration of the speed profile diverged and the fallback
    /// also failed, which should not happen; reported instead of panicking.
    #[error("speed profile did not converge: {0}")]
    ProfileNotConverged(String),

    /// A configuration value is outside its valid range.
    #[error("invalid configuration: {0}")]
    Config(String),

    /// Sensor simulation was asked for parameters it cannot honour.
    #[error("invalid sensor configuration: {0}")]
    SensorConfig(String),

    /// The GPU backend failed; callers may retry on the CPU backend.
    #[error("compute backend '{backend}' failed: {message}")]
    Backend {
        backend: &'static str,
        message: String,
    },

    /// No compute adapter of the requested kind is available.
    #[error("no GPU adapter available{detail}")]
    NoAdapter { detail: String },

    /// Serialisation of an output artefact failed.
    #[error("failed to write {path}: {source}")]
    Export {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

impl CoreError {
    /// Builds a [`CoreError::Config`] from anything printable.
    pub fn config(message: impl Into<String>) -> Self {
        CoreError::Config(message.into())
    }

    /// Builds a [`CoreError::UnusablePosition`].
    pub fn unusable(x: f64, y: f64, reason: impl Into<String>) -> Self {
        CoreError::UnusablePosition {
            x,
            y,
            reason: reason.into(),
        }
    }
}
