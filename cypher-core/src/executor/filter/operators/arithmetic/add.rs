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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CypherDuration;

    #[test]
    fn add_null_left() {
        assert_eq!(add(Value::Null, Value::Int(1)).unwrap(), Value::Null);
    }
    #[test]
    fn add_null_right() {
        assert_eq!(add(Value::Int(1), Value::Null).unwrap(), Value::Null);
    }
    #[test]
    fn add_int_int() {
        assert_eq!(add(Value::Int(2), Value::Int(3)).unwrap(), Value::Int(5));
    }
    #[test]
    fn add_int_overflow() {
        assert!(add(Value::Int(i64::MAX), Value::Int(1)).is_err());
    }
    #[test]
    fn add_float_float() {
        assert_eq!(
            add(Value::Float(1.5), Value::Float(2.5)).unwrap(),
            Value::Float(4.0)
        );
    }
    #[test]
    fn add_int_float() {
        assert_eq!(
            add(Value::Int(1), Value::Float(2.5)).unwrap(),
            Value::Float(3.5)
        );
    }
    #[test]
    fn add_float_int() {
        assert_eq!(
            add(Value::Float(2.5), Value::Int(1)).unwrap(),
            Value::Float(3.5)
        );
    }
    #[test]
    fn add_str_str() {
        let r = add(Value::Str("ab".into()), Value::Str("cd".into())).unwrap();
        assert_eq!(r, Value::Str("abcd".into()));
    }
    #[test]
    fn add_list_list() {
        let r = add(
            Value::List(vec![Value::Int(1)]),
            Value::List(vec![Value::Int(2)]),
        )
        .unwrap();
        if let Value::List(items) = r {
            assert_eq!(items.len(), 2)
        } else {
            panic!()
        }
    }
    #[test]
    fn add_list_element_appends() {
        let r = add(Value::List(vec![Value::Int(1)]), Value::Int(2)).unwrap();
        if let Value::List(items) = r {
            assert_eq!(items, vec![Value::Int(1), Value::Int(2)]);
        } else {
            panic!()
        }
    }
    #[test]
    fn add_element_list_prepends() {
        let r = add(Value::Int(0), Value::List(vec![Value::Int(1)])).unwrap();
        if let Value::List(items) = r {
            assert_eq!(items, vec![Value::Int(0), Value::Int(1)]);
        } else {
            panic!()
        }
    }
    #[test]
    fn add_duration_duration() {
        let d = CypherDuration {
            months: 0,
            days: 0,
            nanos: 1_000_000_000,
        };
        let r = add(Value::Duration(d), Value::Duration(d)).unwrap();
        if let Value::Duration(d) = r {
            assert_eq!(d.nanos, 2_000_000_000)
        } else {
            panic!()
        }
    }
    #[test]
    fn add_invalid_types_errors() {
        assert!(add(Value::Bool(true), Value::Int(1)).is_err());
    }
}
