use gql_core::error::*;

/// Map a GqlError to a (code, message) tuple for Bolt FAILURE responses.
///
/// Error codes follow the Neo4j status code taxonomy so that standard Neo4j
/// drivers surface recognisable error categories to application code.
pub fn gql_error_to_bolt(err: &GqlError) -> (String, String) {
    match err {
        GqlError::Parse(_) => (
            "Neo.ClientError.Statement.SyntaxError".to_string(),
            err.to_string(),
        ),
        GqlError::Plan(e) => match e {
            PlanError::UndefinedVariable { .. } => (
                "Neo.ClientError.Statement.SyntaxError".to_string(),
                e.to_string(),
            ),
            PlanError::SemanticError { .. } => (
                "Neo.ClientError.Statement.SemanticError".to_string(),
                e.to_string(),
            ),
            _ => (
                "Neo.ClientError.Statement.SyntaxError".to_string(),
                e.to_string(),
            ),
        },
        GqlError::Execution(e) => match e {
            ExecutionError::TypeMismatch { actual, .. } if actual == "unbound" => (
                "Neo.ClientError.Statement.ParameterMissing".to_string(),
                e.to_string(),
            ),
            ExecutionError::TypeMismatch { .. } | ExecutionError::InvalidOperator { .. } => (
                "Neo.ClientError.Statement.TypeError".to_string(),
                e.to_string(),
            ),
            ExecutionError::Timeout { .. } => (
                "Neo.TransientError.Transaction.Timeout".to_string(),
                e.to_string(),
            ),
            ExecutionError::UnsupportedFeature { .. } => (
                "Neo.ClientError.Statement.NotSystemDatabaseError".to_string(),
                e.to_string(),
            ),
            _ => (
                "Neo.DatabaseError.Statement.ExecutionFailed".to_string(),
                e.to_string(),
            ),
        },
        GqlError::Storage(e) => match e {
            StorageError::GraphNotFound { .. } => (
                "Neo.ClientError.Database.DatabaseNotFound".to_string(),
                e.to_string(),
            ),
            StorageError::GraphAlreadyExists { .. } => (
                "Neo.ClientError.Database.ExistingDatabaseFound".to_string(),
                e.to_string(),
            ),
            StorageError::TransactionConflict => (
                "Neo.TransientError.Transaction.DeadlockDetected".to_string(),
                e.to_string(),
            ),
            StorageError::DeleteGraphTooLarge { name, .. } => (
                "Neo.ClientError.Database.DeleteGraphTooLarge".to_string(),
                format!("graph '{name}' is too large for atomic deletion; use selective deletion instead"),
            ),
            _ => (
                "Neo.DatabaseError.General.UnknownError".to_string(),
                e.to_string(),
            ),
        },
        GqlError::Type(_) => (
            "Neo.ClientError.Statement.TypeError".to_string(),
            err.to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_maps_to_syntax_error() {
        let err = GqlError::Parse(ParseError::EmptyQuery);
        let (code, _msg) = gql_error_to_bolt(&err);
        assert_eq!(code, "Neo.ClientError.Statement.SyntaxError");
    }

    #[test]
    fn undefined_variable_maps_to_syntax_error() {
        let err = GqlError::Plan(PlanError::UndefinedVariable {
            name: "x".to_string(),
            offset: 0,
        });
        let (code, _msg) = gql_error_to_bolt(&err);
        assert_eq!(code, "Neo.ClientError.Statement.SyntaxError");
    }

    #[test]
    fn semantic_error_maps_to_semantic_error() {
        let err = GqlError::Plan(PlanError::SemanticError {
            message: "bad".to_string(),
        });
        let (code, _msg) = gql_error_to_bolt(&err);
        assert_eq!(code, "Neo.ClientError.Statement.SemanticError");
    }

    #[test]
    fn type_mismatch_maps_to_type_error() {
        let err = GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "Int".to_string(),
            actual: "String".to_string(),
        });
        let (code, _msg) = gql_error_to_bolt(&err);
        assert_eq!(code, "Neo.ClientError.Statement.TypeError");
    }

    #[test]
    fn timeout_maps_to_transient_timeout() {
        let err = GqlError::Execution(ExecutionError::Timeout {
            elapsed_ms: 100,
            limit_ms: 50,
        });
        let (code, _msg) = gql_error_to_bolt(&err);
        assert_eq!(code, "Neo.TransientError.Transaction.Timeout");
    }

    #[test]
    fn graph_not_found_maps_to_database_not_found() {
        let err = GqlError::Storage(StorageError::GraphNotFound {
            name: "missing".to_string(),
        });
        let (code, _msg) = gql_error_to_bolt(&err);
        assert_eq!(code, "Neo.ClientError.Database.DatabaseNotFound");
    }

    #[test]
    fn graph_already_exists_maps_to_existing_database() {
        let err = GqlError::Storage(StorageError::GraphAlreadyExists {
            name: "dup".to_string(),
        });
        let (code, _msg) = gql_error_to_bolt(&err);
        assert_eq!(code, "Neo.ClientError.Database.ExistingDatabaseFound");
    }

    #[test]
    fn transaction_conflict_maps_to_deadlock() {
        let err = GqlError::Storage(StorageError::TransactionConflict);
        let (code, _msg) = gql_error_to_bolt(&err);
        assert_eq!(code, "Neo.TransientError.Transaction.DeadlockDetected");
    }

    #[test]
    fn generic_storage_error_maps_to_unknown() {
        let err = GqlError::Storage(StorageError::Backend {
            msg: "disk full".to_string(),
        });
        let (code, _msg) = gql_error_to_bolt(&err);
        assert_eq!(code, "Neo.DatabaseError.General.UnknownError");
    }

    #[test]
    fn type_error_maps_to_type_error() {
        let err = GqlError::Type(TypeError::CannotCoerce {
            from: "Int".to_string(),
            to: "String".to_string(),
        });
        let (code, _msg) = gql_error_to_bolt(&err);
        assert_eq!(code, "Neo.ClientError.Statement.TypeError");
    }

    #[test]
    fn unsupported_feature_maps_to_not_system_db() {
        let err = GqlError::Execution(ExecutionError::UnsupportedFeature {
            feature: "CALL subquery".to_string(),
        });
        let (code, _msg) = gql_error_to_bolt(&err);
        assert_eq!(code, "Neo.ClientError.Statement.NotSystemDatabaseError");
    }

    #[test]
    fn division_by_zero_maps_to_execution_failed() {
        let err = GqlError::Execution(ExecutionError::DivisionByZero);
        let (code, _msg) = gql_error_to_bolt(&err);
        assert_eq!(code, "Neo.DatabaseError.Statement.ExecutionFailed");
    }

    #[test]
    fn unbound_parameter_maps_to_parameter_missing() {
        let err = GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "parameter '$x' to be bound".to_string(),
            actual: "unbound".to_string(),
        });
        let (code, _msg) = gql_error_to_bolt(&err);
        assert_eq!(code, "Neo.ClientError.Statement.ParameterMissing");
    }

    #[test]
    fn error_message_is_preserved() {
        let err = GqlError::Parse(ParseError::EmptyQuery);
        let (_code, msg) = gql_error_to_bolt(&err);
        assert!(msg.contains("empty query"));
    }
}

// ── WS-F (#755): DeleteGraphTooLarge → Bolt client-error code ─────────────────
//
// These tests will FAIL TO COMPILE until:
//   (a) StorageError::DeleteGraphTooLarge is added to gql-core/src/error.rs
//   (b) gql_error_to_bolt() gains an explicit arm for DeleteGraphTooLarge
//       before the _ catch-all in the GqlError::Storage(e) => match e { ... } block
//
// The test here closes the "D" sub-scenario in
// tests/qa/gadugi/issue-755-oversized-delete-error-contract.yaml.

#[cfg(test)]
mod ws_f_delete_graph_too_large_bolt_tests {
    use super::*;

    /// Verify that StorageError::DeleteGraphTooLarge maps to the correct Bolt
    /// client-error code and that the message includes the graph name but does
    /// NOT expose resource_count or limit to Bolt clients.
    ///
    /// FAILS TO COMPILE until StorageError::DeleteGraphTooLarge variant exists
    /// and gql_error_to_bolt() has an explicit arm for it.
    #[test]
    fn delete_graph_too_large_maps_to_client_error() {
        let err = GqlError::Storage(StorageError::DeleteGraphTooLarge {
            name: "bigdb".to_string(),
            resource: "nodes",
            resource_count: 500_000,
            limit: 100_000,
        });
        let (code, msg) = gql_error_to_bolt(&err);
        assert_eq!(
            code, "Neo.ClientError.Database.DeleteGraphTooLarge",
            "DeleteGraphTooLarge must produce Neo.ClientError.Database.DeleteGraphTooLarge \
             (a client error, not a database/unknown error), got: {code}"
        );
        // The Bolt message must identify the graph to the operator.
        assert!(
            msg.contains("bigdb"),
            "Bolt error message must include the graph name 'bigdb', got: {msg}"
        );
        // Resource counts and limits must not leak to Bolt clients.
        assert!(
            !msg.contains("500000"),
            "Bolt error message must NOT expose resource_count (500000), got: {msg}"
        );
        assert!(
            !msg.contains("100000"),
            "Bolt error message must NOT expose limit (100000), got: {msg}"
        );
    }

    /// Verify that DeleteGraphTooLarge does NOT fall through to the generic
    /// Neo.DatabaseError.General.UnknownError catch-all in gql_error_to_bolt().
    ///
    /// The _ catch-all exists for truly unknown storage errors.  DeleteGraphTooLarge
    /// is a defined client error and must have its own explicit arm so drivers can
    /// distinguish it from infrastructure failures.
    ///
    /// FAILS TO COMPILE until StorageError::DeleteGraphTooLarge variant exists.
    #[test]
    fn delete_graph_too_large_is_not_unknown_error() {
        let err = GqlError::Storage(StorageError::DeleteGraphTooLarge {
            name: "g".to_string(),
            resource: "edges",
            resource_count: 200_001,
            limit: 100_000,
        });
        let (code, _msg) = gql_error_to_bolt(&err);
        assert_ne!(
            code, "Neo.DatabaseError.General.UnknownError",
            "DeleteGraphTooLarge must NOT fall through to the UnknownError catch-all; \
             add an explicit arm before the _ wildcard in gql_error_to_bolt()"
        );
    }
}
