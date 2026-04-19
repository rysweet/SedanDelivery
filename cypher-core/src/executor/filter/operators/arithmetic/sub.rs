//! `-` operator: numeric subtraction and temporal/duration subtraction.

use crate::error::{ExecutionError, GqlError};
use crate::executor::filter::temporal::{duration_sub, temporal_add_duration};
use crate::types::Value;

pub(crate) fn sub(lv: Value, rv: Value) -> Result<Value, GqlError> {
    match (lv, rv) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.checked_sub(b).ok_or_else(|| {
            GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "integer arithmetic without overflow".to_string(),
                actual: "integer overflow".to_string(),
            })
        })?)),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 - b)),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a - b as f64)),
        // Temporal - Duration
        (
            ref temporal @ (Value::Date(_)
            | Value::LocalTime(_)
            | Value::Time(_, _)
            | Value::LocalDateTime(_)
            | Value::DateTime(_)),
            Value::Duration(ref dur),
        ) => temporal_add_duration(temporal, dur, true),
        // Duration - Duration
        (Value::Duration(a), Value::Duration(b)) => Ok(Value::Duration(duration_sub(&a, &b))),
        (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
            op: "-".to_string(),
            left: l.type_name().to_string(),
            right: r.type_name().to_string(),
        })),
    }
}
