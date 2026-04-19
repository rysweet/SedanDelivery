use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum StorageError {
    #[error("database '{name}' does not exist")]
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

    #[error("constraint violation: {msg}")]
    ConstraintViolation { msg: String },

    #[error("graph '{name}' has {resource_count} {resource} exceeding the limit of {limit}")]
    DeleteGraphTooLarge {
        name: String,
        resource: &'static str,
        resource_count: usize,
        limit: usize,
    },

    #[error("no property index for label '{label}' property '{property}' — create an index or use a full scan")]
    NoPropertyIndex { label: String, property: String },

    #[error("configuration error: {msg}")]
    Configuration { msg: String },
}

impl StorageError {
    pub fn backend(msg: impl Into<String>) -> Self {
        Self::Backend { msg: msg.into() }
    }
}
