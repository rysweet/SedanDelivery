//! `From` impls mapping `cypher_core` errors to [`ApiError`] with appropriate
//! HTTP status codes.

use axum::http::StatusCode;

use super::types::ApiError;

/// Convert a `cypher_core::error::GqlError` into an API error.
impl From<cypher_core::error::GqlError> for ApiError {
    fn from(e: cypher_core::error::GqlError) -> Self {
        use cypher_core::error::{GqlError, StorageError};
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
            GqlError::Storage(StorageError::DeleteGraphTooLarge { name, .. }) => {
                ApiError::payload_too_large(
                    "graph_too_large_for_delete",
                    format!("graph '{name}' is too large for atomic deletion; use selective deletion instead"),
                )
            }
            GqlError::Storage(StorageError::LockFenced { .. }) => {
                // Fence-token rejection (F1, issue #1769): a stalled writer
                // tried to land late writes after its lease was stolen. The
                // operation is fail-closed and the client may retry.
                ApiError::new(StatusCode::SERVICE_UNAVAILABLE, e.to_string())
            }
            GqlError::Storage(StorageError::TransactionConflict) => {
                ApiError::new(StatusCode::CONFLICT, e.to_string())
            }
            GqlError::Storage(StorageError::ConstraintViolation { .. }) => {
                ApiError::new(StatusCode::CONFLICT, e.to_string())
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
                use cypher_core::error::ExecutionError;
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

/// Convert a `cypher_core::error::StorageError` into an API error.
impl From<cypher_core::error::StorageError> for ApiError {
    fn from(e: cypher_core::error::StorageError) -> Self {
        ApiError::from(cypher_core::error::GqlError::Storage(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cypher_core::error::{ExecutionError, GqlError, ParseError, PlanError, StorageError};

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

// ── WS-F (#755): DeleteGraphTooLarge → HTTP 413 ──────────────────────────────
//
// These tests close the "C" sub-scenario in
// tests/qa/gadugi/issue-755-oversized-delete-error-contract.yaml.

#[cfg(test)]
mod ws_f_delete_graph_too_large_api_tests {
    use super::*;
    use crate::error::ErrorBody;
    use cypher_core::error::{GqlError, StorageError};

    /// Verify that StorageError::DeleteGraphTooLarge produces HTTP 413 PAYLOAD_TOO_LARGE.
    #[test]
    fn delete_graph_too_large_maps_to_payload_too_large() {
        let e = GqlError::Storage(StorageError::DeleteGraphTooLarge {
            name: "bigdb".to_string(),
            resource: "nodes",
            resource_count: 999_999,
            limit: 100_000,
        });
        let api_err = ApiError::from(e);
        assert_eq!(
            api_err.status,
            StatusCode::PAYLOAD_TOO_LARGE,
            "DeleteGraphTooLarge must map to HTTP 413 PAYLOAD_TOO_LARGE, got: {}",
            api_err.status
        );
        assert_eq!(
            api_err.message, "graph_too_large_for_delete",
            "HTTP 413 error field must be the canonical token 'graph_too_large_for_delete', \
             got: {}",
            api_err.message
        );
    }

    /// Verify the HTTP 413 response body does NOT leak resource_count or limit values.
    ///
    /// Information-disclosure prevention: counts and limits are internal housekeeping
    /// values that must not be surfaced to API callers (SEC-6 / AC-3 from issue #755).
    #[test]
    fn delete_graph_too_large_does_not_leak_resource_count_in_response() {
        let e = GqlError::Storage(StorageError::DeleteGraphTooLarge {
            name: "secretgraph".to_string(),
            resource: "nodes",
            resource_count: 999_999,
            limit: 100_000,
        });
        let api_err = ApiError::from(e);

        // The error token must be the fixed canonical string — not a formatted
        // message that includes counts, limits, or internal constant names.
        assert_eq!(
            api_err.message, "graph_too_large_for_delete",
            "message field must be canonical token, got: {}",
            api_err.message
        );

        // Serialize the full ErrorBody to JSON and verify no sensitive values appear.
        let body = serde_json::to_string(&ErrorBody {
            error: api_err.message.clone(),
            message: api_err.detail.clone(),
        })
        .expect("ErrorBody must be JSON-serializable");

        assert!(
            !body.contains("999999"),
            "HTTP 413 response must NOT expose resource_count (999999), got: {body}"
        );
        assert!(
            !body.contains("100000"),
            "HTTP 413 response must NOT expose limit (100000), got: {body}"
        );
        assert!(
            !body.contains("MAX_NODES"),
            "HTTP 413 response must NOT reference the MAX_NODES constant name, got: {body}"
        );
        assert!(
            body.contains("graph_too_large_for_delete"),
            "HTTP 413 response must include canonical error token 'graph_too_large_for_delete', \
             got: {body}"
        );
        assert!(
            body.contains("secretgraph"),
            "HTTP 413 detail message must include the graph name 'secretgraph', got: {body}"
        );
    }
}
