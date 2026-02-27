//! Shared error type that converts to an Axum JSON response.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

/// Wire format for all error responses: `{ "error": "..." }`.
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub error: String,
}

/// A typed API error that knows its HTTP status code.
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, msg: impl Into<String>) -> Self {
        Self {
            status,
            message: msg.into(),
        }
    }

    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: msg.into(),
        }
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: msg.into(),
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: msg.into(),
        }
    }

    pub fn unprocessable(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            message: msg.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                error: self.message,
            }),
        )
            .into_response()
    }
}

/// Convert a `gql_core::error::GqlError` into an API error.
impl From<gql_core::error::GqlError> for ApiError {
    fn from(e: gql_core::error::GqlError) -> Self {
        use gql_core::error::{GqlError, StorageError};
        match &e {
            GqlError::Storage(StorageError::GraphNotFound { .. }) => {
                ApiError::not_found(e.to_string())
            }
            GqlError::Storage(StorageError::GraphAlreadyExists { .. }) => {
                ApiError::conflict(e.to_string())
            }
            GqlError::Storage(StorageError::NodeNotFound { .. }) => {
                ApiError::not_found(e.to_string())
            }
            GqlError::Storage(StorageError::EdgeNotFound { .. }) => {
                ApiError::not_found(e.to_string())
            }
            GqlError::Storage(StorageError::InvalidGraphName { .. }) => {
                ApiError::bad_request(e.to_string())
            }
            GqlError::Parse(_) => ApiError::bad_request(e.to_string()),
            GqlError::Plan(_) => ApiError::bad_request(e.to_string()),
            GqlError::Execution(exec_err) => {
                use gql_core::error::ExecutionError;
                match exec_err {
                    ExecutionError::Timeout { .. } => {
                        ApiError::new(StatusCode::REQUEST_TIMEOUT, e.to_string())
                    }
                    ExecutionError::ResultSetTooLarge { .. } => {
                        ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, e.to_string())
                    }
                    ExecutionError::UnsupportedFeature { .. } => {
                        ApiError::new(StatusCode::NOT_IMPLEMENTED, e.to_string())
                    }
                    _ => ApiError::unprocessable(e.to_string()),
                }
            }
            _ => ApiError::unprocessable(e.to_string()),
        }
    }
}

/// Convert a `gql_core::error::StorageError` into an API error.
impl From<gql_core::error::StorageError> for ApiError {
    fn from(e: gql_core::error::StorageError) -> Self {
        ApiError::from(gql_core::error::GqlError::Storage(e))
    }
}
