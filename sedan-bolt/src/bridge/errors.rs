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
