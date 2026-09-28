//! Error type of the importer.

use std::path::PathBuf;

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, GeoError>;

/// Every failure mode of fetching, decoding or assembling map source data.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GeoError {
    /// Underlying I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A dataset could not be retrieved.
    #[error("request to {url} failed: {source}")]
    Http {
        /// Fully qualified URL that was requested.
        url: String,
        /// Transport or status failure reported by the HTTP client.
        #[source]
        source: reqwest::Error,
    },

    /// An endpoint answered, but not with the dataset.
    ///
    /// Overpass reports a busy dispatcher and a rejected query in the same way
    /// on the wire, so the answer is kept: it is the only thing that tells the
    /// two apart.
    #[error("{endpoint} answered HTTP {status}: {detail}")]
    Response {
        /// Endpoint that was queried.
        endpoint: String,
        /// Status code it answered with.
        status: u16,
        /// Beginning of the answer, with runs of whitespace collapsed.
        detail: String,
    },

    /// A tile could not be decoded.
    #[error("cannot decode {path}: {source}")]
    Tiff {
        /// File the decoder was reading.
        path: PathBuf,
        /// Failure reported by the TIFF decoder.
        #[source]
        source: tiff::TiffError,
    },

    /// A response body was not the JSON it promised to be.
    #[error("invalid JSON in {what}: {source}")]
    Json {
        /// Human-readable name of the payload being parsed.
        what: String,
        /// Failure reported by the JSON parser.
        #[source]
        source: serde_json::Error,
    },

    /// The container refused the assembled image.
    #[error("omf error: {0}")]
    Map(#[from] ourealis_map_format::MapError),

    /// The request is not a usable map specification.
    #[error("invalid import specification: {0}")]
    Invalid(String),

    /// A source dataset is missing or carries nothing usable.
    #[error("unusable source data: {0}")]
    Data(String),
}

impl GeoError {
    /// Builds a [`GeoError::Invalid`] from anything printable.
    pub fn invalid(message: impl Into<String>) -> Self {
        GeoError::Invalid(message.into())
    }

    /// Builds a [`GeoError::Data`] from anything printable.
    pub fn data(message: impl Into<String>) -> Self {
        GeoError::Data(message.into())
    }

    /// Builds a [`GeoError::Response`], trimming the answer to a readable
    /// excerpt.
    pub fn response(endpoint: &str, status: u16, body: &str) -> Self {
        GeoError::Response {
            endpoint: endpoint.to_owned(),
            status,
            detail: excerpt(body),
        }
    }
}

/// Length an excerpted answer is cut to.
const EXCERPT_CHARS: usize = 240;

/// Beginning of a response body, with runs of whitespace collapsed.
fn excerpt(body: &str) -> String {
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= EXCERPT_CHARS {
        return collapsed;
    }
    let mut text: String = collapsed.chars().take(EXCERPT_CHARS).collect();
    text.push_str("...");
    text
}
