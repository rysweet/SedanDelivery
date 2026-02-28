/// Error types for the sedan-client crate.

#[derive(Debug, thiserror::Error)]
pub enum SedanError {
    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Protocol error: {0}")]
    Protocol(String),
}
