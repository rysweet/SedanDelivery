//! Type coercion builtins: toString, toInteger, toFloat, toBoolean.

use crate::error::GqlError;
use crate::types::Value;

use super::helpers::{require_arity, type_error};

pub(in crate::executor::filter) fn builtin_tostring(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("toString", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        // A String is already a string — return it as-is (no extra quotes).
        Value::Str(s) => Ok(Value::Str(s.clone())),
        Value::Int(_) | Value::Float(_) | Value::Bool(_) => {
            Ok(Value::Str(vals[0].to_string().into_boxed_str()))
        }
        Value::Date(_)
        | Value::LocalTime(_)
        | Value::Time(_, _)
        | Value::LocalDateTime(_)
        | Value::DateTime(_)
        | Value::Duration(_) => Ok(Value::Str(vals[0].to_string().into_boxed_str())),
        other => type_error("toString", other),
    }
}

pub(in crate::executor::filter) fn builtin_tointeger(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("toInteger", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(f) => {
            if f.is_finite() {
                Ok(Value::Int(*f as i64))
            } else {
                Ok(Value::Null)
            }
        }
        Value::Str(s) => {
            // Try integer parse first, then float parse (truncating).
            // TCK expects NULL for truly non-numeric strings.
            if let Ok(n) = s.parse::<i64>() {
                Ok(Value::Int(n))
            } else if let Ok(f) = s.parse::<f64>() {
                if f.is_finite() {
                    Ok(Value::Int(f as i64))
                } else {
                    Ok(Value::Null)
                }
            } else {
                Ok(Value::Null)
            }
        }
        other => type_error("toInteger", other),
    }
}

pub(in crate::executor::filter) fn builtin_tofloat(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("toFloat", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Float(f) => Ok(Value::Float(*f)),
        Value::Int(n) => Ok(Value::Float(*n as f64)),
        Value::Str(s) => {
            // TCK expects NULL for non-numeric strings, not an error.
            Ok(s.parse::<f64>().map(Value::Float).unwrap_or(Value::Null))
        }
        other => type_error("toFloat", other),
    }
}

pub(in crate::executor::filter) fn builtin_toboolean(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("toBoolean", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Bool(b) => Ok(Value::Bool(*b)),
        Value::Str(s) => match s.to_ascii_lowercase().as_str() {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            // Per Cypher spec: toBoolean() on an unrecognized string returns null,
            // not an error. This is intentional Cypher-compliant behavior.
            _ => Ok(Value::Null),
        },
        other => type_error("toBoolean", other),
    }
}
