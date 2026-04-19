//! Helpers for extracting node references from `Value`s and constructing
//! variable-scope errors.

use crate::error::{ExecutionError, GqlError};
use crate::storage::NodeId;
use crate::types::Value;

/// Extract a `NodeId` from a `Value::Node(id)`, returning an error if the
/// value is not a node reference.
pub(crate) fn extract_node_id(val: &Value, var: &str) -> Result<NodeId, GqlError> {
    match val {
        Value::Node(id) => Ok(*id),
        other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: format!("Node for variable '{var}'"),
            actual: other.type_name().to_string(),
        })),
    }
}

/// Build an error for an undefined variable.
pub(crate) fn undefined_var(name: &str) -> GqlError {
    GqlError::Execution(ExecutionError::TypeMismatch {
        expected: format!("variable '{name}' in scope"),
        actual: "undefined".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_node_id_success() {
        assert_eq!(
            extract_node_id(&Value::Node(NodeId(42)), "n").unwrap(),
            NodeId(42)
        );
    }

    #[test]
    fn extract_node_id_wrong_type() {
        assert!(extract_node_id(&Value::Int(1), "n").is_err());
    }
}
