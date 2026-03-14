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
#[derive(Debug)]
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
            GqlError::Storage(StorageError::Backend { .. }) => {
                // Sanitize the backend error message at the HTTP API boundary.
                // In release builds the raw Redis error string (which may contain
                // cluster topology such as "MOVED 1234 10.0.0.1:6379") is replaced
                // with an opaque message so internal infrastructure details are never
                // exposed in HTTP 500 responses (SEC-6). Debug builds retain the full
                // message for local diagnostics.
                #[cfg(not(debug_assertions))]
                let msg = "internal storage error".to_string();
                #[cfg(debug_assertions)]
                let msg = e.to_string();
                ApiError::internal(msg)
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use gql_core::error::{ExecutionError, GqlError, ParseError, PlanError, StorageError};

    // ── Constructor tests ────────────────────────────────────────────────────

    #[test]
    fn bad_request_constructor() {
        let e = ApiError::bad_request("invalid input");
        assert_eq!(e.status, StatusCode::BAD_REQUEST);
        assert_eq!(e.message, "invalid input");
    }

    #[test]
    fn not_found_constructor() {
        let e = ApiError::not_found("not here");
        assert_eq!(e.status, StatusCode::NOT_FOUND);
        assert_eq!(e.message, "not here");
    }

    #[test]
    fn conflict_constructor() {
        let e = ApiError::conflict("already exists");
        assert_eq!(e.status, StatusCode::CONFLICT);
        assert_eq!(e.message, "already exists");
    }

    #[test]
    fn internal_constructor() {
        let e = ApiError::internal("boom");
        assert_eq!(e.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(e.message, "boom");
    }

    #[test]
    fn unprocessable_constructor() {
        let e = ApiError::unprocessable("bad entity");
        assert_eq!(e.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(e.message, "bad entity");
    }

    #[test]
    fn new_constructor_custom_status() {
        let e = ApiError::new(StatusCode::IM_A_TEAPOT, "teapot");
        assert_eq!(e.status, StatusCode::IM_A_TEAPOT);
        assert_eq!(e.message, "teapot");
    }

    // ── From<GqlError> conversion tests ──────────────────────────────────────

    #[test]
    fn from_gql_graph_not_found() {
        let e = GqlError::Storage(StorageError::GraphNotFound {
            name: "missing".to_string(),
        });
        let api_err = ApiError::from(e);
        assert_eq!(api_err.status, StatusCode::NOT_FOUND);
        assert!(api_err.message.contains("missing"));
    }

    #[test]
    fn from_gql_graph_already_exists() {
        let e = GqlError::Storage(StorageError::GraphAlreadyExists {
            name: "dup".to_string(),
        });
        let api_err = ApiError::from(e);
        assert_eq!(api_err.status, StatusCode::CONFLICT);
    }

    #[test]
    fn from_gql_node_not_found() {
        let e = GqlError::Storage(StorageError::NodeNotFound { id: 42 });
        let api_err = ApiError::from(e);
        assert_eq!(api_err.status, StatusCode::NOT_FOUND);
        assert!(api_err.message.contains("42"));
    }

    #[test]
    fn from_gql_edge_not_found() {
        let e = GqlError::Storage(StorageError::EdgeNotFound { id: 99 });
        let api_err = ApiError::from(e);
        assert_eq!(api_err.status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn from_gql_invalid_graph_name() {
        let e = GqlError::Storage(StorageError::InvalidGraphName {
            name: "bad:name".to_string(),
            reason: "contains colon",
        });
        let api_err = ApiError::from(e);
        assert_eq!(api_err.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn from_gql_parse_error() {
        let e = GqlError::Parse(ParseError::EmptyQuery);
        let api_err = ApiError::from(e);
        assert_eq!(api_err.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn from_gql_plan_error() {
        let e = GqlError::Plan(PlanError::ReturnStarNoVariables);
        let api_err = ApiError::from(e);
        assert_eq!(api_err.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn from_gql_execution_timeout() {
        let e = GqlError::Execution(ExecutionError::Timeout {
            elapsed_ms: 5000,
            limit_ms: 3000,
        });
        let api_err = ApiError::from(e);
        assert_eq!(api_err.status, StatusCode::REQUEST_TIMEOUT);
    }

    #[test]
    fn from_gql_execution_result_too_large() {
        let e = GqlError::Execution(ExecutionError::ResultSetTooLarge {
            count: 100_000,
            limit: 10_000,
        });
        let api_err = ApiError::from(e);
        assert_eq!(api_err.status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn from_gql_execution_unsupported_feature() {
        let e = GqlError::Execution(ExecutionError::UnsupportedFeature {
            feature: "CALL subquery".to_string(),
        });
        let api_err = ApiError::from(e);
        assert_eq!(api_err.status, StatusCode::NOT_IMPLEMENTED);
    }

    #[test]
    fn from_gql_execution_generic() {
        let e = GqlError::Execution(ExecutionError::DivisionByZero);
        let api_err = ApiError::from(e);
        assert_eq!(api_err.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    // ── From<StorageError> shortcut ──────────────────────────────────────────

    #[test]
    fn from_storage_error_directly() {
        let e = StorageError::GraphNotFound {
            name: "test".to_string(),
        };
        let api_err = ApiError::from(e);
        assert_eq!(api_err.status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn from_storage_error_backend() {
        let e = StorageError::Backend {
            msg: "redis down".to_string(),
        };
        let api_err = ApiError::from(e);
        assert_eq!(api_err.status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn backend_error_message_sanitized_in_release() {
        // In release builds the raw backend message (which may contain Redis
        // cluster topology such as "MOVED 1234 10.0.0.1:6379") must not appear
        // in the HTTP response body (SEC-6).  In debug builds the full message
        // is preserved so engineers can diagnose issues locally.
        let topology_detail = "MOVED 1234 10.0.0.1:6379";
        let e = StorageError::Backend {
            msg: topology_detail.to_string(),
        };
        let api_err = ApiError::from(e);
        assert_eq!(api_err.status, StatusCode::INTERNAL_SERVER_ERROR);

        #[cfg(not(debug_assertions))]
        assert!(
            !api_err.message.contains(topology_detail),
            "release build must not expose Redis topology in HTTP 500: {}",
            api_err.message
        );

        #[cfg(debug_assertions)]
        assert!(
            api_err.message.contains(topology_detail),
            "debug build must preserve backend detail for diagnostics: {}",
            api_err.message
        );
    }
}
