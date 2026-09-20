//! Service error type and its wire classification.
//!
//! One error type for the whole crate, so a handler can return a `core` failure,
//! a map-format failure or a rejected request without wrapping it first. The
//! facade modules map [`ErrorKind`] onto an HTTP status and a gRPC status code;
//! the message text is passed through unchanged and stays English.

use ourealis_core::CoreError;
use ourealis_map_format::MapError;

/// Result alias of this crate.
pub type Result<T, E = ServiceError> = std::result::Result<T, E>;

/// What a failure means to the caller, independent of the transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The request is malformed or carries an unusable value.
    Invalid,
    /// The request is well formed but cannot be satisfied: no route between the
    /// given points, a start position inside a building, an empty candidate set.
    Unprocessable,
    /// The addressed resource does not exist.
    NotFound,
    /// The request conflicts with the current state.
    Conflict,
    /// The payload is larger than the configured limit.
    TooLarge,
    /// The service is at capacity and the caller may retry.
    Busy,
    /// The request asks for something this build cannot do.
    Unsupported,
    /// A failure inside the simulator or the map container.
    Core,
    /// Anything else: a bug or an environmental failure.
    Internal,
}

impl ErrorKind {
    /// Machine-readable kind used in JSON bodies and gRPC metadata.
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Invalid => "invalid",
            ErrorKind::Unprocessable => "unprocessable",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Conflict => "conflict",
            ErrorKind::TooLarge => "too_large",
            ErrorKind::Busy => "busy",
            ErrorKind::Unsupported => "unsupported",
            ErrorKind::Core => "core",
            ErrorKind::Internal => "internal",
        }
    }
}

/// A service failure with its classification.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// A configuration file, or a value inside one, is not usable.
    #[error("{0}")]
    Config(String),
    /// The request is malformed.
    #[error("{0}")]
    Invalid(String),
    /// The request cannot be satisfied.
    #[error("{0}")]
    Unprocessable(String),
    /// The addressed resource does not exist.
    #[error("{0}")]
    NotFound(String),
    /// The request conflicts with the current state.
    #[error("{0}")]
    Conflict(String),
    /// The payload exceeds the configured limit.
    #[error("{0}")]
    TooLarge(String),
    /// The service is at capacity.
    #[error("{0}")]
    Busy(String),
    /// The request asks for something unsupported.
    #[error("{0}")]
    Unsupported(String),
    /// The simulator failed.
    #[error(transparent)]
    Core(#[from] CoreError),
    /// The map container failed on bytes that came from the request, so the caller can
    /// act on it.
    #[error(transparent)]
    Map(#[from] MapError),
    /// A map already in the library could not be read back.
    ///
    /// The difference from [`ServiceError::Map`] is whose fault it is: an image that
    /// was accepted into the library and then fails to parse or checksum is a storage
    /// problem, and reporting it as a bad request would tell the caller to fix a
    /// request that was never wrong.
    #[error("map {id} is stored but cannot be read: {message}")]
    StoredMap {
        /// Identifier of the library entry.
        id: String,
        /// The container's own message.
        message: String,
    },
    /// Reading or writing a configuration file, map image or result file failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A request or response body could not be serialised.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// A configuration file could not be parsed.
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
    /// Anything else.
    #[error("{0}")]
    Internal(String),
}

impl ServiceError {
    /// Classification used by the facades.
    pub fn kind(&self) -> ErrorKind {
        match self {
            ServiceError::Config(_) | ServiceError::Invalid(_) => ErrorKind::Invalid,
            ServiceError::Unprocessable(_) => ErrorKind::Unprocessable,
            ServiceError::NotFound(_) => ErrorKind::NotFound,
            ServiceError::Conflict(_) => ErrorKind::Conflict,
            ServiceError::TooLarge(_) => ErrorKind::TooLarge,
            ServiceError::Busy(_) => ErrorKind::Busy,
            ServiceError::Unsupported(_) => ErrorKind::Unsupported,
            ServiceError::Internal(_) => ErrorKind::Internal,
            ServiceError::Io(_) | ServiceError::Json(_) | ServiceError::Toml(_) => {
                ErrorKind::Internal
            }
            ServiceError::Core(error) => core_kind(error),
            ServiceError::Map(error) => map_kind(error),
            ServiceError::StoredMap { .. } => ErrorKind::Internal,
        }
    }

    /// True when the failure came from the simulator or a map container, which the
    /// wire format reports as one class with the original message.
    pub fn is_core(&self) -> bool {
        matches!(
            self,
            ServiceError::Core(_) | ServiceError::Map(_) | ServiceError::StoredMap { .. }
        )
    }

    /// Reclassifies a container failure as a storage failure, when it is one.
    ///
    /// Used on the read-back path, where the bytes came from the library rather than
    /// from the request. Only *corruption* counts: a layer that does not exist, a
    /// layer kind that has no chunks, or a codec this build cannot read are statements
    /// about the request, and turning them into a server fault would tell the caller
    /// to look for a problem in the wrong place.
    pub fn from_stored_map(id: &str, error: MapError) -> Self {
        if is_request_fault(&error) {
            return ServiceError::Map(error);
        }
        ServiceError::StoredMap {
            id: id.to_string(),
            message: error.to_string(),
        }
    }
}

/// Classifies a simulator failure.
///
/// A missing layer or a dimension mismatch is a property of the map image, so it
/// is reported as a bad request rather than as a server fault: the caller picked
/// the map, and the message names what is missing.
fn core_kind(error: &CoreError) -> ErrorKind {
    match error {
        CoreError::UnusablePosition { .. }
        | CoreError::NoPath { .. }
        | CoreError::ProfileNotConverged(_) => ErrorKind::Unprocessable,
        CoreError::MissingLayer { .. }
        | CoreError::DimensionMismatch { .. }
        | CoreError::Config(_)
        | CoreError::SensorConfig(_) => ErrorKind::Invalid,
        CoreError::Map(inner) => map_kind(inner),
        // A GPU that is not there is a capability of the host, not a defect in
        // the request, so it is reported as a server-side failure.
        CoreError::Backend { .. } | CoreError::NoAdapter { .. } | CoreError::Export { .. } => {
            ErrorKind::Internal
        }
        // `CoreError` is `non_exhaustive`, so a variant added in `core` reads as
        // a simulator failure rather than failing to compile here.
        _ => ErrorKind::Core,
    }
}

/// True when a container failure is a statement about the *request* rather than about
/// the stored bytes.
///
/// Deliberately an allowlist of request faults, not a denylist of corruptions: the
/// reader reports a damaged file through several variants, including `Invalid` for its
/// structural checks (a header pointing at a footer offset the length contradicts), so
/// enumerating corruption would silently miss the cases that matter most. Anything not
/// named here, read back from a library entry, is treated as a storage failure.
fn is_request_fault(error: &MapError) -> bool {
    matches!(
        error,
        MapError::LayerNotFound { .. }
            | MapError::NotACellLayer { .. }
            | MapError::UnsupportedCodec { .. }
            | MapError::StaleDerived { .. }
    )
}

/// Classifies a map container failure.
fn map_kind(error: &MapError) -> ErrorKind {
    match error {
        MapError::Invalid(_)
        | MapError::NotACellLayer { .. }
        | MapError::BadMagic { .. }
        | MapError::UnsupportedVersion { .. }
        | MapError::HeaderCrc { .. }
        | MapError::ChunkCrc { .. }
        | MapError::FileHash { .. }
        | MapError::Truncated { .. }
        | MapError::MissingTlv { .. }
        | MapError::LayerNotFound { .. } => ErrorKind::Invalid,
        MapError::UnsupportedCodec { .. } | MapError::StaleDerived { .. } => ErrorKind::Unsupported,
        MapError::PatchBaseMismatch { .. } | MapError::PatchDerivedLayer { .. } => {
            ErrorKind::Conflict
        }
        _ => ErrorKind::Core,
    }
}

impl ServiceError {
    /// Convenience constructor for a missing resource.
    pub fn not_found(what: impl std::fmt::Display) -> Self {
        ServiceError::NotFound(format!("{what} not found"))
    }
}
