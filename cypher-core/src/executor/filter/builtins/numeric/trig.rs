//! Trigonometric builtins: `sin`, `cos`, `tan`, inverses, `atan2`,
//! `degrees`, `radians`, `haversin`, and the constants `pi`, `e`.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::helpers::{require_arity, type_error};

pub(in crate::executor::filter) fn builtin_sin(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("sin", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).sin())),
        Value::Float(f) => Ok(Value::Float(f.sin())),
        other => type_error("sin", other),
    }
}

pub(in crate::executor::filter) fn builtin_cos(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("cos", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).cos())),
        Value::Float(f) => Ok(Value::Float(f.cos())),
        other => type_error("cos", other),
    }
}

pub(in crate::executor::filter) fn builtin_tan(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("tan", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).tan())),
        Value::Float(f) => Ok(Value::Float(f.tan())),
        other => type_error("tan", other),
    }
}

pub(in crate::executor::filter) fn builtin_asin(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("asin", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).asin())),
        Value::Float(f) => Ok(Value::Float(f.asin())),
        other => type_error("asin", other),
    }
}

pub(in crate::executor::filter) fn builtin_acos(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("acos", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).acos())),
        Value::Float(f) => Ok(Value::Float(f.acos())),
        other => type_error("acos", other),
    }
}

pub(in crate::executor::filter) fn builtin_atan(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("atan", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).atan())),
        Value::Float(f) => Ok(Value::Float(f.atan())),
        other => type_error("atan", other),
    }
}

pub(in crate::executor::filter) fn builtin_atan2(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("atan2", 2, &vals)?;
    match (&vals[0], &vals[1]) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::Int(y), Value::Int(x)) => Ok(Value::Float((*y as f64).atan2(*x as f64))),
        (Value::Float(y), Value::Float(x)) => Ok(Value::Float(y.atan2(*x))),
        (Value::Int(y), Value::Float(x)) => Ok(Value::Float((*y as f64).atan2(*x))),
        (Value::Float(y), Value::Int(x)) => Ok(Value::Float(y.atan2(*x as f64))),
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "Numeric, Numeric".to_string(),
            actual: format!("{}, {}", vals[0].type_name(), vals[1].type_name()),
        })),
    }
}

pub(in crate::executor::filter) fn builtin_degrees(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("degrees", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).to_degrees())),
        Value::Float(f) => Ok(Value::Float(f.to_degrees())),
        other => type_error("degrees", other),
    }
}

pub(in crate::executor::filter) fn builtin_radians(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("radians", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).to_radians())),
        Value::Float(f) => Ok(Value::Float(f.to_radians())),
        other => type_error("radians", other),
    }
}

pub(in crate::executor::filter) fn builtin_haversin(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("haversin", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => {
            let x = *n as f64;
            Ok(Value::Float((1.0 - x.cos()) / 2.0))
        }
        Value::Float(f) => Ok(Value::Float((1.0 - f.cos()) / 2.0)),
        other => type_error("haversin", other),
    }
}

pub(in crate::executor::filter) fn builtin_pi(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("pi", 0, &vals)?;
    Ok(Value::Float(std::f64::consts::PI))
}

pub(in crate::executor::filter) fn builtin_e(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("e", 0, &vals)?;
    Ok(Value::Float(std::f64::consts::E))
}
