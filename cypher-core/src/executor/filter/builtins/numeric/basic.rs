//! Basic numeric builtins: `abs`, `ceil`, `floor`, `round`, `sqrt`, `sign`.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::helpers::{require_arity, type_error};

pub(in crate::executor::filter) fn builtin_abs(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("abs", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Int(n.checked_abs().ok_or_else(|| {
            GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "integer arithmetic without overflow".to_string(),
                actual: "integer overflow".to_string(),
            })
        })?)),
        Value::Float(f) => Ok(Value::Float(f.abs())),
        other => type_error("abs", other),
    }
}

pub(in crate::executor::filter) fn builtin_ceil(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("ceil", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(f) => Ok(Value::Float(f.ceil())),
        other => type_error("ceil", other),
    }
}

pub(in crate::executor::filter) fn builtin_floor(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("floor", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(f) => Ok(Value::Float(f.floor())),
        other => type_error("floor", other),
    }
}

pub(in crate::executor::filter) fn builtin_round(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("round", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(f) => Ok(Value::Float(f.round())),
        other => type_error("round", other),
    }
}

pub(in crate::executor::filter) fn builtin_sqrt(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("sqrt", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).sqrt())),
        Value::Float(f) => Ok(Value::Float(f.sqrt())),
        other => type_error("sqrt", other),
    }
}

pub(in crate::executor::filter) fn builtin_sign(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("sign", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Int(n.signum())),
        Value::Float(f) => Ok(Value::Float(f.signum())),
        other => type_error("sign", other),
    }
}
