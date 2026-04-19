//! Exponential / logarithmic builtins: `log`, `log10`, `exp`.

use crate::error::GqlError;
use crate::types::Value;

use super::super::helpers::{require_arity, type_error};

pub(in crate::executor::filter) fn builtin_log(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("log", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).ln())),
        Value::Float(f) => Ok(Value::Float(f.ln())),
        other => type_error("log", other),
    }
}

pub(in crate::executor::filter) fn builtin_log10(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("log10", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).log10())),
        Value::Float(f) => Ok(Value::Float(f.log10())),
        other => type_error("log10", other),
    }
}

pub(in crate::executor::filter) fn builtin_exp(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("exp", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).exp())),
        Value::Float(f) => Ok(Value::Float(f.exp())),
        other => type_error("exp", other),
    }
}
