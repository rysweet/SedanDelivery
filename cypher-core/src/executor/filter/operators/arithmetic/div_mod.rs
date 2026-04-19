//! Division-flavoured operators: `/` (div), `%` (modulo), and `^` (power).
//!
//! Grouped together because they share the float-by-zero and IEEE 754
//! semantics required by the Cypher TCK.

use crate::error::{ExecutionError, GqlError};
use crate::executor::filter::temporal::{duration_div, duration_mul_f64};
use crate::types::Value;

pub(crate) fn div(lv: Value, rv: Value) -> Result<Value, GqlError> {
    match (lv, rv) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::Int(_), Value::Int(0)) => Err(GqlError::Execution(ExecutionError::DivisionByZero)),
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a / b)),
        // Float division by zero follows IEEE 754: returns Inf, -Inf, or NaN.
        // The TCK expects this behavior (e.g. 0.0/0.0 → NaN).
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a / b)),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 / b)),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a / b as f64)),
        // Duration / Integer
        (Value::Duration(d), Value::Int(n)) => duration_div(&d, n).map(Value::Duration),
        // Duration / Float: duration / f = duration * (1/f)
        (Value::Duration(d), Value::Float(f)) => Ok(Value::Duration(duration_mul_f64(&d, 1.0 / f))),
        (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
            op: "/".to_string(),
            left: l.type_name().to_string(),
            right: r.type_name().to_string(),
        })),
    }
}

pub(crate) fn modulo(lv: Value, rv: Value) -> Result<Value, GqlError> {
    match (lv, rv) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::Int(_), Value::Int(0)) => Err(GqlError::Execution(ExecutionError::DivisionByZero)),
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a % b)),
        // Float modulo by zero follows IEEE 754: returns NaN.
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a % b)),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 % b)),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a % b as f64)),
        (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
            op: "%".to_string(),
            left: l.type_name().to_string(),
            right: r.type_name().to_string(),
        })),
    }
}

pub(crate) fn pow(lv: Value, rv: Value) -> Result<Value, GqlError> {
    match (lv, rv) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        // ^ always returns Float per Cypher spec.
        (Value::Int(a), Value::Int(b)) => Ok(Value::Float((a as f64).powf(b as f64))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a.powf(b))),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float((a as f64).powf(b))),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a.powf(b as f64))),
        (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
            op: "^".to_string(),
            left: l.type_name().to_string(),
            right: r.type_name().to_string(),
        })),
    }
}
