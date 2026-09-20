//! Error mapping for both facades.
//!
//! One classification, two encodings: JSON with an HTTP status for the HTTP
//! facade, a gRPC [`tonic::Status`] for the RPC facade. The message text is passed
//! through unchanged and stays English.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::error::{ErrorKind, ServiceError};

/// Wire shape of a failure.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ErrorBody {
    /// The failure.
    pub error: ErrorDetail,
}

/// Detail of a failure.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ErrorDetail {
    /// Machine-readable kind, see [`ErrorKind::as_str`].
    pub kind: String,
    /// Human-readable message, always English.
    pub message: String,
    /// HTTP status the failure was reported with, absent on the gRPC side.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

impl ErrorBody {
    /// Builds a body from an error.
    pub fn from_error(error: &ServiceError) -> Self {
        let kind = error.kind();
        Self {
            error: ErrorDetail {
                kind: kind.as_str().to_string(),
                message: error.to_string(),
                status: Some(http_status(kind).as_u16()),
            },
        }
    }
}

/// HTTP status for a classification.
pub fn http_status(kind: ErrorKind) -> StatusCode {
    match kind {
        ErrorKind::Invalid => StatusCode::BAD_REQUEST,
        ErrorKind::Unprocessable => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::Conflict => StatusCode::CONFLICT,
        ErrorKind::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorKind::Busy => StatusCode::SERVICE_UNAVAILABLE,
        ErrorKind::Unsupported => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        // A failure inside the simulator or the map container is reported as a
        // server fault when the caller cannot act on it and as a bad request when
        // the message names something the caller chose wrong; `error::core_kind`
        // decides which, so this arm only sees the genuine faults.
        ErrorKind::Core | ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// gRPC status code for a classification.
pub fn grpc_code(kind: ErrorKind) -> tonic::Code {
    match kind {
        ErrorKind::Invalid => tonic::Code::InvalidArgument,
        ErrorKind::Unprocessable => tonic::Code::FailedPrecondition,
        ErrorKind::NotFound => tonic::Code::NotFound,
        // A conflict is a state precondition that the call contradicts (cancelling
        // a finished job, patching against a foreign base file), not an attempt to
        // create something that exists: `AlreadyExists` would misdescribe it.
        ErrorKind::Conflict => tonic::Code::FailedPrecondition,
        ErrorKind::TooLarge => tonic::Code::ResourceExhausted,
        ErrorKind::Busy => tonic::Code::Unavailable,
        ErrorKind::Unsupported => tonic::Code::Unimplemented,
        ErrorKind::Core | ErrorKind::Internal => tonic::Code::Internal,
    }
}

/// Converts a service error into a gRPC status.
///
/// The kind travels in the `ourealis-error-kind` metadata key so a client can
/// branch on it without parsing the message.
pub fn to_status(error: &ServiceError) -> tonic::Status {
    let kind = error.kind();
    let status = tonic::Status::new(grpc_code(kind), error.to_string());
    match tonic::metadata::MetadataValue::try_from(kind.as_str()) {
        Ok(value) => {
            let mut status = status;
            status.metadata_mut().insert("ourealis-error-kind", value);
            status
        }
        Err(_) => status,
    }
}

impl IntoResponse for ServiceError {
    fn into_response(self) -> Response {
        let status = http_status(self.kind());
        if status.is_server_error() {
            tracing::error!("request failed: {self}");
        } else {
            tracing::debug!("request rejected: {self}");
        }
        (status, Json(ErrorBody::from_error(&self))).into_response()
    }
}

/// Turns a JSON extraction failure into a classified error.
///
/// `axum`'s own rejection text names the byte offset, which is what a client needs
/// to fix its payload, so it is kept and only the classification is added.
pub fn json_rejection(error: axum::extract::rejection::JsonRejection) -> ServiceError {
    ServiceError::Invalid(format!("request body is not usable: {error}"))
}

/// Turns a query extraction failure into a classified error.
pub fn query_rejection(error: axum::extract::rejection::QueryRejection) -> ServiceError {
    ServiceError::Invalid(format!("query parameters are not usable: {error}"))
}
