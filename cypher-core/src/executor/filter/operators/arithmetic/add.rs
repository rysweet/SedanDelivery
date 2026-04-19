//! `+` operator: numeric addition, string/list concatenation, and
//! temporal/duration addition.

use crate::error::{ExecutionError, GqlError};
use crate::executor::filter::temporal::{duration_add, temporal_add_duration};
use crate::types::Value;

pub(crate) fn add(lv: Value, rv: Value) -> Result<Value, GqlError> {
    match (lv, rv) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.checked_add(b).ok_or_else(|| {
            GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "integer arithmetic without overflow".to_string(),
                actual: "integer overflow".to_string(),
            })
        })?)),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 + b)),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a + b as f64)),
        (Value::Str(a), Value::Str(b)) => Ok(Value::Str(
            (a.as_ref().to_string() + b.as_ref()).into_boxed_str(),
        )),
        (Value::List(mut a), Value::List(b)) => {
            a.extend(b);
            Ok(Value::List(a))
        }
        // Temporal + Duration
        (
            ref temporal @ (Value::Date(_)
            | Value::LocalTime(_)
            | Value::Time(_, _)
            | Value::LocalDateTime(_)
            | Value::DateTime(_)),
            Value::Duration(ref dur),
        ) => temporal_add_duration(temporal, dur, false),
        // Duration + Temporal
        (
            Value::Duration(ref dur),
            ref temporal @ (Value::Date(_)
            | Value::LocalTime(_)
            | Value::Time(_, _)
            | Value::LocalDateTime(_)
            | Value::DateTime(_)),
        ) => temporal_add_duration(temporal, dur, false),
        // Duration + Duration
        (Value::Duration(a), Value::Duration(b)) => Ok(Value::Duration(duration_add(&a, &b))),
        // List + element → append
        (Value::List(mut a), val) => {
            a.push(val);
            Ok(Value::List(a))
        }
        // element + List → prepend
        (val, Value::List(mut b)) => {
            b.insert(0, val);
            Ok(Value::List(b))
        }
        (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
            op: "+".to_string(),
            left: l.type_name().to_string(),
            right: r.type_name().to_string(),
        })),
    }
}
