//! Shared helpers: arity validation and uniform type-error construction.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

pub(in crate::executor::filter) fn require_arity(
    name: &str,
    expected: usize,
    vals: &[Value],
) -> Result<(), GqlError> {
    if vals.len() != expected {
        Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: format!("{name}() expects {expected} argument(s)"),
            actual: format!("{} argument(s)", vals.len()),
        }))
    } else {
        Ok(())
    }
}

pub(in crate::executor::filter) fn type_error(name: &str, val: &Value) -> Result<Value, GqlError> {
    Err(GqlError::Execution(ExecutionError::TypeMismatch {
        expected: format!("{name}() argument"),
        actual: val.type_name().to_string(),
    }))
}
