use thiserror::Error;

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

// ── Parse errors ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Error)]
pub enum ParseError {
    #[error("syntax error at byte {offset}: expected {expected}, found {found}")]
    Syntax {
        offset: usize,
        expected: String,
        found: String,
    },

    #[error("unterminated string literal starting at byte {offset}")]
    UnterminatedString { offset: usize },

    #[error("integer overflow in literal '{literal}' at byte {offset}")]
    IntegerOverflow { literal: String, offset: usize },

    #[error("invalid escape sequence '\\{char}' at byte {offset}")]
    InvalidEscape { char: char, offset: usize },

    #[error("query too large: {size} bytes, limit is {limit} bytes")]
    QueryTooLarge { size: usize, limit: usize },

    #[error("expression nesting depth exceeded limit of {limit}")]
    RecursionLimitExceeded { limit: u32 },

    #[error("empty query")]
    EmptyQuery,
}

// ── Planning errors ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Error)]
pub enum PlanError {
    #[error("undefined variable '{name}' at position {offset}")]
    UndefinedVariable { name: String, offset: usize },

    #[error("ambiguous variable '{name}': defined in multiple MATCH patterns")]
    AmbiguousVariable { name: String },

    #[error("cannot use aggregating function '{name}' outside of WITH or RETURN")]
    AggregateOutsideReturn { name: String },

    #[error("RETURN * requires at least one variable in scope")]
    ReturnStarNoVariables,

    #[error("ORDER BY references '{name}' which is not in RETURN projection")]
    OrderByNotInReturn { name: String },

    #[error("write clause '{clause}' cannot follow read-only clause in the same query")]
    WriteAfterRead { clause: String },

    /// General semantic error: a query violates a semantic rule (type conflict, invalid
    /// aggregation placement, constraint violation, etc.).  Replaces the previous abuse
    /// of `UndefinedVariable` as a catch-all for non-undefined-variable errors.
    #[error("{message}")]
    SemanticError { message: String },
}

// ── Execution errors ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Error)]
pub enum ExecutionError {
    #[error("type mismatch: expected {expected}, got {actual}")]
    TypeMismatch { expected: String, actual: String },

    #[error("cannot apply operator '{op}' to types {left} and {right}")]
    InvalidOperator {
        op: String,
        left: String,
        right: String,
    },

    #[error("division by zero")]
    DivisionByZero,

    #[error("list index {index} out of bounds (list has {len} elements)")]
    IndexOutOfBounds { index: i64, len: usize },

    #[error("result set too large: {count} rows, limit is {limit}")]
    ResultSetTooLarge { count: usize, limit: usize },

    #[error("query timed out after {elapsed_ms}ms (limit: {limit_ms}ms)")]
    Timeout { elapsed_ms: u64, limit_ms: u64 },

    #[error("feature not yet implemented: {feature}")]
    UnsupportedFeature { feature: String },

    #[error("variable-length path exceeded max depth {max_depth}")]
    PathDepthExceeded { max_depth: u32 },

    #[error("node #{id} not found during traversal (graph may have been modified concurrently)")]
    NodeNotFound { id: u64 },
}

// ── Storage errors ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Error)]
pub enum StorageError {
    #[error("graph '{name}' not found")]
    GraphNotFound { name: String },

    #[error("graph '{name}' already exists")]
    GraphAlreadyExists { name: String },

    #[error("invalid graph name '{name}': {reason}")]
    InvalidGraphName { name: String, reason: &'static str },

    #[error("node #{id} not found")]
    NodeNotFound { id: u64 },

    #[error("edge #{id} not found")]
    EdgeNotFound { id: u64 },

    #[error("ID overflow: graph '{graph}' has reached maximum node/edge count")]
    IdOverflow { graph: String },

    #[error("backend error: {msg}")]
    Backend { msg: String },

    #[error("serialization error: {msg}")]
    Serialization { msg: String },

    #[error("transaction conflict: concurrent write detected, retry the operation")]
    TransactionConflict,
}

// ── Type errors ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Error)]
pub enum TypeError {
    #[error("cannot coerce {from} to {to}")]
    CannotCoerce { from: String, to: String },

    #[error("'{value}' is not a valid {type_name}")]
    InvalidLiteral { value: String, type_name: String },
}

// ── Convenience constructors ──────────────────────────────────────────────────

impl ExecutionError {
    pub fn unsupported(feature: impl Into<String>) -> Self {
        Self::UnsupportedFeature {
            feature: feature.into(),
        }
    }
}

impl StorageError {
    pub fn backend(msg: impl Into<String>) -> Self {
        Self::Backend { msg: msg.into() }
    }
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

    // ── Error path tests ─────────────────────────────────────────────────────

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
        // Wrap in GqlError to verify the From conversion works.
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
        // Wrap in GqlError to verify the From conversion works.
        let gql: GqlError = e.into();
        assert!(matches!(
            gql,
            GqlError::Storage(StorageError::EdgeNotFound { id: 42 })
        ));
    }
}
