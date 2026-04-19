//! Null-handling builtins: coalesce, isNull.

use crate::error::GqlError;
use crate::types::Value;

use super::helpers::require_arity;

pub(in crate::executor::filter) fn builtin_coalesce(vals: Vec<Value>) -> Result<Value, GqlError> {
    // Return first non-null argument.
    for v in vals {
        if !v.is_null() {
            return Ok(v);
        }
    }
    Ok(Value::Null)
}

pub(in crate::executor::filter) fn builtin_isnull(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("isNull", 1, &vals)?;
    Ok(Value::Bool(vals[0].is_null()))
}
