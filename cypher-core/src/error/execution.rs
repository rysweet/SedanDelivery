use thiserror::Error;

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

    #[error("integer overflow: {context}")]
    IntegerOverflow { context: String },
}

impl ExecutionError {
    pub fn unsupported(feature: impl Into<String>) -> Self {
        Self::UnsupportedFeature {
            feature: feature.into(),
        }
    }
}
