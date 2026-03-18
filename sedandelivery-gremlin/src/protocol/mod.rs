//! Gremlin protocol handling — request parsing, dispatch, and response framing.

pub mod handler;
pub mod request;
pub mod response;

/// Errors that can occur during protocol handling.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("malformed request: {0}")]
    MalformedRequest(String),

    #[error("execution error: {0}")]
    #[allow(dead_code)]
    Execution(String),
}
