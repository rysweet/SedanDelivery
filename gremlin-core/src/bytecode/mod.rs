//! Gremlin bytecode representation and deserialization from GraphSON.
//!
//! This module is split into focused submodules:
//! - [`step`] — the [`GremlinStep`] enum and the [`Pop`] modifier
//! - [`predicate`] — the [`Predicate`] enum for comparison operations
//! - [`deserialize`] — GraphSON JSON → typed step deserialization

use crate::types::GremlinValue;
use serde::{Deserialize, Serialize};

mod deserialize;
mod predicate;
mod step;

#[cfg(test)]
mod extra_tests;
#[cfg(test)]
mod nested_tests;
#[cfg(test)]
mod tests;

pub use deserialize::deserialize_bytecode;
pub use predicate::Predicate;
pub use step::{GremlinStep, Pop};

/// Raw Gremlin bytecode as sent over the wire (GraphSON format).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bytecode {
    #[serde(default)]
    pub source_instructions: Vec<(String, Vec<GremlinValue>)>,
    #[serde(default)]
    pub step_instructions: Vec<(String, Vec<GremlinValue>)>,
}

/// Errors that can occur during bytecode deserialization.
#[derive(Debug, thiserror::Error)]
pub enum BytecodeError {
    #[error("unknown step name: {0}")]
    UnknownStep(String),
    #[error("invalid argument for step '{step}': {reason}")]
    InvalidArgument { step: String, reason: String },
    #[error("missing required argument for step '{0}'")]
    MissingArgument(String),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
}
