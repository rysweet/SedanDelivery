//! Helpers for parsing nested sub-traversal arguments.

use super::deserialize_bytecode;
use crate::bytecode::{BytecodeError, GremlinStep};

/// Parse a single nested-traversal argument (either bare `{"step":[...]}` or
/// GraphSON-wrapped `{"@type":"g:Bytecode","@value":{"step":[...]}}`).
pub(in crate::bytecode) fn parse_inner_traversal(
    step_name: &str,
    arg: &serde_json::Value,
) -> Result<Vec<GremlinStep>, BytecodeError> {
    let inner = arg.get("@value").unwrap_or(arg);
    deserialize_bytecode(inner).map_err(|e| BytecodeError::InvalidArgument {
        step: step_name.into(),
        reason: format!("nested traversal: {e}"),
    })
}

/// Parse all args as nested traversals. Used by steps like `union`, `and`,
/// `or`, `coalesce`, `match` that accept a variadic list of sub-traversals.
pub(in crate::bytecode) fn parse_inner_traversals(
    step_name: &str,
    args: &[serde_json::Value],
) -> Result<Vec<Vec<GremlinStep>>, BytecodeError> {
    args.iter()
        .map(|a| parse_inner_traversal(step_name, a))
        .collect()
}

/// Parse the single-traversal argument, returning MissingArgument if absent.
pub(in crate::bytecode) fn parse_single_inner_traversal(
    step_name: &str,
    args: &[serde_json::Value],
) -> Result<Vec<GremlinStep>, BytecodeError> {
    let arg = args
        .first()
        .ok_or_else(|| BytecodeError::MissingArgument(step_name.into()))?;
    parse_inner_traversal(step_name, arg)
}
