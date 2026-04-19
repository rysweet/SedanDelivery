use thiserror::Error;

mod execution;
mod parse;
mod plan;
mod storage;
mod types;

pub use execution::ExecutionError;
pub use parse::ParseError;
pub use plan::PlanError;
pub use storage::StorageError;
pub use types::TypeError;

/// Top-level error type for the entire GQL engine.
///
/// Each variant maps to a Redis error prefix convention:
/// - `Parse`     → "ERR"
/// - `Plan`      → "ERR"
/// - `Execution` → "ERR"
/// - `Storage`   → "ERR" or "WRONGTYPE"
/// - `Type`      → "WRONGTYPE"
/// - `Timeout`   → "ERR timeout"
#[derive(Debug, Clone, Error)]
pub enum GqlError {
    #[error("parse error: {0}")]
    Parse(#[from] ParseError),

    #[error("planning error: {0}")]
    Plan(#[from] PlanError),

    #[error("execution error: {0}")]
    Execution(#[from] ExecutionError),

    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("type error: {0}")]
    Type(#[from] TypeError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_are_informative() {
        let e = ParseError::Syntax {
            offset: 10,
            expected: "identifier".into(),
            found: "123".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("byte 10"));
        assert!(msg.contains("identifier"));
        assert!(msg.contains("123"));
    }

    #[test]
    fn gql_error_from_parse_error() {
        let e: GqlError = ParseError::EmptyQuery.into();
        assert!(matches!(e, GqlError::Parse(ParseError::EmptyQuery)));
    }

    #[test]
    fn unsupported_feature_convenience() {
        let e = ExecutionError::unsupported("CALL subquery");
        assert!(e.to_string().contains("CALL subquery"));
    }

    #[test]
    fn query_too_large_error_message() {
        let e = ParseError::QueryTooLarge {
            size: 2_000_000,
            limit: 1_000_000,
        };
        let msg = e.to_string();
        assert!(
            msg.contains("2000000"),
            "message should include actual size"
        );
        assert!(msg.contains("1000000"), "message should include limit");
    }

    #[test]
    fn index_out_of_bounds_error_message() {
        let e = ExecutionError::IndexOutOfBounds { index: 5, len: 3 };
        let msg = e.to_string();
        assert!(msg.contains('5'), "message should include the index");
        assert!(msg.contains('3'), "message should include the length");
    }

    #[test]
    fn path_depth_exceeded_error_message() {
        let e = ExecutionError::PathDepthExceeded { max_depth: 100 };
        let msg = e.to_string();
        assert!(msg.contains("100"), "message should include the max depth");
    }

    #[test]
    fn ambiguous_variable_error_message() {
        let e = PlanError::AmbiguousVariable {
            name: "n".to_string(),
        };
        let msg = e.to_string();
        assert!(
            msg.contains("'n'"),
            "message should include the variable name"
        );
        let gql: GqlError = e.into();
        assert!(matches!(
            gql,
            GqlError::Plan(PlanError::AmbiguousVariable { .. })
        ));
    }

    #[test]
    fn edge_not_found_error_message() {
        let e = StorageError::EdgeNotFound { id: 42 };
        let msg = e.to_string();
        assert!(msg.contains("42"), "message should include the edge id");
        let gql: GqlError = e.into();
        assert!(matches!(
            gql,
            GqlError::Storage(StorageError::EdgeNotFound { id: 42 })
        ));
    }
}

#[cfg(test)]
mod ws_f_delete_graph_too_large_tests {
    use super::*;

    #[test]
    fn delete_graph_too_large_constructs_with_node_resource() {
        let err = StorageError::DeleteGraphTooLarge {
            name: "bigdb".to_string(),
            resource: "nodes",
            resource_count: 150_000,
            limit: 100_000,
        };
        assert!(
            matches!(err, StorageError::DeleteGraphTooLarge { .. }),
            "DeleteGraphTooLarge must be a StorageError variant"
        );
    }

    #[test]
    fn delete_graph_too_large_constructs_with_edge_resource() {
        let err = StorageError::DeleteGraphTooLarge {
            name: "bigdb".to_string(),
            resource: "edges",
            resource_count: 200_001,
            limit: 100_000,
        };
        assert!(
            matches!(
                err,
                StorageError::DeleteGraphTooLarge {
                    resource: "edges",
                    ..
                }
            ),
            "resource field must be accessible for matching"
        );
    }

    #[test]
    fn delete_graph_too_large_message_is_informative() {
        let err = StorageError::DeleteGraphTooLarge {
            name: "hugegraph".to_string(),
            resource: "nodes",
            resource_count: 500_000,
            limit: 100_000,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("hugegraph"),
            "error message must include graph name, got: {msg}"
        );
        assert!(
            msg.contains("500000") || msg.contains("500_000"),
            "error message must include resource count, got: {msg}"
        );
        assert!(
            msg.contains("100000") || msg.contains("100_000"),
            "error message must include the limit, got: {msg}"
        );
        assert!(
            msg.contains("nodes"),
            "error message must include the resource type, got: {msg}"
        );
    }

    #[test]
    fn delete_graph_too_large_wraps_into_gql_error() {
        let storage_err = StorageError::DeleteGraphTooLarge {
            name: "x".to_string(),
            resource: "nodes",
            resource_count: 1,
            limit: 0,
        };
        let gql: GqlError = storage_err.into();
        assert!(
            matches!(
                gql,
                GqlError::Storage(StorageError::DeleteGraphTooLarge { .. })
            ),
            "StorageError::DeleteGraphTooLarge must become GqlError::Storage"
        );
    }

    #[test]
    fn delete_graph_too_large_name_field_is_correct() {
        let err = StorageError::DeleteGraphTooLarge {
            name: "correct_graph".to_string(),
            resource: "nodes",
            resource_count: 200_000,
            limit: 100_000,
        };
        if let StorageError::DeleteGraphTooLarge { name, .. } = err {
            assert_eq!(name, "correct_graph");
        } else {
            panic!("expected DeleteGraphTooLarge variant");
        }
    }

    #[test]
    fn delete_graph_too_large_count_and_limit_accessible() {
        let err = StorageError::DeleteGraphTooLarge {
            name: "g".to_string(),
            resource: "nodes",
            resource_count: 150_001,
            limit: 150_000,
        };
        if let StorageError::DeleteGraphTooLarge {
            resource_count,
            limit,
            ..
        } = err
        {
            assert!(
                resource_count > limit,
                "resource_count ({resource_count}) must exceed limit ({limit})"
            );
        } else {
            panic!("expected DeleteGraphTooLarge variant");
        }
    }
}
