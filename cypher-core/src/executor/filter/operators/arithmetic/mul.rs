//! `*` operator: numeric multiplication and duration scaling.

use crate::error::{ExecutionError, GqlError};
use crate::executor::filter::temporal::{duration_mul, duration_mul_f64};
use crate::types::Value;

pub(crate) fn mul(lv: Value, rv: Value) -> Result<Value, GqlError> {
    match (lv, rv) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.checked_mul(b).ok_or_else(|| {
            GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "integer arithmetic without overflow".to_string(),
                actual: "integer overflow".to_string(),
            })
        })?)),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 * b)),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a * b as f64)),
        // Duration * Integer
        (Value::Duration(d), Value::Int(n)) => Ok(Value::Duration(duration_mul(&d, n))),
        (Value::Int(n), Value::Duration(d)) => Ok(Value::Duration(duration_mul(&d, n))),
        // Duration * Float: use rational approximation for clean fractions like 0.5
        (Value::Duration(d), Value::Float(f)) => Ok(Value::Duration(duration_mul_f64(&d, f))),
        (Value::Float(f), Value::Duration(d)) => Ok(Value::Duration(duration_mul_f64(&d, f))),
        (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
            op: "*".to_string(),
            left: l.type_name().to_string(),
            right: r.type_name().to_string(),
        })),
    }
}
