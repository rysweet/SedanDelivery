//! Arithmetic operators: `+`, `-`, `*`, `/`, `%`, `^`.
//!
//! Includes string/list concatenation (sharing the `+` arm) and
//! temporal/duration arithmetic.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use crate::executor::filter::temporal::{
    duration_add, duration_div, duration_mul, duration_mul_f64, duration_sub, temporal_add_duration,
};

pub(super) fn add(lv: Value, rv: Value) -> Result<Value, GqlError> {
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

pub(super) fn sub(lv: Value, rv: Value) -> Result<Value, GqlError> {
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

pub(super) fn mul(lv: Value, rv: Value) -> Result<Value, GqlError> {
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

pub(super) fn div(lv: Value, rv: Value) -> Result<Value, GqlError> {
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

pub(super) fn modulo(lv: Value, rv: Value) -> Result<Value, GqlError> {
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

pub(super) fn pow(lv: Value, rv: Value) -> Result<Value, GqlError> {
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

#[cfg(test)]
mod tests {
    use super::super::test_utils::{bin, lit_f, lit_i, lit_null, lit_s, run};
    use crate::ast::{BinaryOp, Expr};
    use crate::types::Value;

    #[test]
    fn add_ints() {
        assert_eq!(
            run(&bin(BinaryOp::Add, lit_i(2), lit_i(3))).unwrap(),
            Value::Int(5)
        );
    }
    #[test]
    fn add_int_overflow() {
        assert!(run(&bin(BinaryOp::Add, lit_i(i64::MAX), lit_i(1))).is_err());
    }
    #[test]
    fn add_floats_mixed() {
        assert_eq!(
            run(&bin(BinaryOp::Add, lit_f(1.5), lit_f(2.5))).unwrap(),
            Value::Float(4.0)
        );
        assert_eq!(
            run(&bin(BinaryOp::Add, lit_i(2), lit_f(0.5))).unwrap(),
            Value::Float(2.5)
        );
        assert_eq!(
            run(&bin(BinaryOp::Add, lit_f(0.5), lit_i(2))).unwrap(),
            Value::Float(2.5)
        );
    }
    #[test]
    fn add_strings() {
        assert_eq!(
            run(&bin(BinaryOp::Add, lit_s("a"), lit_s("b"))).unwrap(),
            Value::Str("ab".into())
        );
    }
    #[test]
    fn add_null() {
        assert_eq!(
            run(&bin(BinaryOp::Add, lit_null(), lit_i(1))).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn add_invalid_err() {
        use crate::ast::Expr;
        assert!(run(&bin(BinaryOp::Add, Expr::Bool(true), Expr::Bool(false))).is_err());
    }
    #[test]
    fn add_list_list() {
        let l = Expr::List(vec![lit_i(1)]);
        let r = Expr::List(vec![lit_i(2)]);
        assert_eq!(
            run(&bin(BinaryOp::Add, l, r)).unwrap(),
            Value::List(vec![Value::Int(1), Value::Int(2)])
        );
    }
    #[test]
    fn add_list_element() {
        let l = Expr::List(vec![lit_i(1)]);
        assert_eq!(
            run(&bin(BinaryOp::Add, l, lit_i(2))).unwrap(),
            Value::List(vec![Value::Int(1), Value::Int(2)])
        );
    }
    #[test]
    fn add_element_list() {
        let r = Expr::List(vec![lit_i(2)]);
        assert_eq!(
            run(&bin(BinaryOp::Add, lit_i(1), r)).unwrap(),
            Value::List(vec![Value::Int(1), Value::Int(2)])
        );
    }
    #[test]
    fn sub_ints() {
        assert_eq!(
            run(&bin(BinaryOp::Sub, lit_i(5), lit_i(3))).unwrap(),
            Value::Int(2)
        );
    }
    #[test]
    fn sub_overflow() {
        assert!(run(&bin(BinaryOp::Sub, lit_i(i64::MIN), lit_i(1))).is_err());
    }
    #[test]
    fn sub_mixed_float() {
        assert_eq!(
            run(&bin(BinaryOp::Sub, lit_f(2.5), lit_i(1))).unwrap(),
            Value::Float(1.5)
        );
        assert_eq!(
            run(&bin(BinaryOp::Sub, lit_i(2), lit_f(0.5))).unwrap(),
            Value::Float(1.5)
        );
        assert_eq!(
            run(&bin(BinaryOp::Sub, lit_f(2.5), lit_f(0.5))).unwrap(),
            Value::Float(2.0)
        );
    }
    #[test]
    fn sub_null() {
        assert_eq!(
            run(&bin(BinaryOp::Sub, lit_null(), lit_i(1))).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn sub_invalid() {
        assert!(run(&bin(BinaryOp::Sub, lit_s("a"), lit_s("b"))).is_err());
    }
    #[test]
    fn mul_ints() {
        assert_eq!(
            run(&bin(BinaryOp::Mul, lit_i(2), lit_i(3))).unwrap(),
            Value::Int(6)
        );
    }
    #[test]
    fn mul_overflow() {
        assert!(run(&bin(BinaryOp::Mul, lit_i(i64::MAX), lit_i(2))).is_err());
    }
    #[test]
    fn mul_floats_mixed() {
        assert_eq!(
            run(&bin(BinaryOp::Mul, lit_f(1.5), lit_f(2.0))).unwrap(),
            Value::Float(3.0)
        );
        assert_eq!(
            run(&bin(BinaryOp::Mul, lit_i(2), lit_f(0.5))).unwrap(),
            Value::Float(1.0)
        );
        assert_eq!(
            run(&bin(BinaryOp::Mul, lit_f(0.5), lit_i(2))).unwrap(),
            Value::Float(1.0)
        );
    }
    #[test]
    fn mul_null() {
        assert_eq!(
            run(&bin(BinaryOp::Mul, lit_null(), lit_i(1))).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn mul_invalid() {
        assert!(run(&bin(BinaryOp::Mul, lit_s("a"), lit_s("b"))).is_err());
    }
    #[test]
    fn div_ints() {
        assert_eq!(
            run(&bin(BinaryOp::Div, lit_i(10), lit_i(3))).unwrap(),
            Value::Int(3)
        );
    }
    #[test]
    fn div_int_by_zero() {
        assert!(run(&bin(BinaryOp::Div, lit_i(1), lit_i(0))).is_err());
    }
    #[test]
    fn div_floats_mixed() {
        assert_eq!(
            run(&bin(BinaryOp::Div, lit_f(1.0), lit_f(2.0))).unwrap(),
            Value::Float(0.5)
        );
        assert_eq!(
            run(&bin(BinaryOp::Div, lit_i(1), lit_f(2.0))).unwrap(),
            Value::Float(0.5)
        );
        assert_eq!(
            run(&bin(BinaryOp::Div, lit_f(1.0), lit_i(2))).unwrap(),
            Value::Float(0.5)
        );
    }
    #[test]
    fn div_null() {
        assert_eq!(
            run(&bin(BinaryOp::Div, lit_null(), lit_i(1))).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn div_invalid() {
        assert!(run(&bin(BinaryOp::Div, lit_s("a"), lit_s("b"))).is_err());
    }
    #[test]
    fn mod_ints() {
        assert_eq!(
            run(&bin(BinaryOp::Mod, lit_i(10), lit_i(3))).unwrap(),
            Value::Int(1)
        );
    }
    #[test]
    fn mod_int_by_zero() {
        assert!(run(&bin(BinaryOp::Mod, lit_i(1), lit_i(0))).is_err());
    }
    #[test]
    fn mod_floats_mixed() {
        assert_eq!(
            run(&bin(BinaryOp::Mod, lit_f(5.0), lit_f(3.0))).unwrap(),
            Value::Float(2.0)
        );
        assert_eq!(
            run(&bin(BinaryOp::Mod, lit_i(5), lit_f(3.0))).unwrap(),
            Value::Float(2.0)
        );
        assert_eq!(
            run(&bin(BinaryOp::Mod, lit_f(5.0), lit_i(3))).unwrap(),
            Value::Float(2.0)
        );
    }
    #[test]
    fn mod_null() {
        assert_eq!(
            run(&bin(BinaryOp::Mod, lit_null(), lit_i(1))).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn mod_invalid() {
        assert!(run(&bin(BinaryOp::Mod, lit_s("a"), lit_s("b"))).is_err());
    }
    #[test]
    fn pow_variants() {
        assert_eq!(
            run(&bin(BinaryOp::Pow, lit_i(2), lit_i(3))).unwrap(),
            Value::Float(8.0)
        );
        assert_eq!(
            run(&bin(BinaryOp::Pow, lit_f(2.0), lit_f(3.0))).unwrap(),
            Value::Float(8.0)
        );
        assert_eq!(
            run(&bin(BinaryOp::Pow, lit_i(2), lit_f(3.0))).unwrap(),
            Value::Float(8.0)
        );
        assert_eq!(
            run(&bin(BinaryOp::Pow, lit_f(2.0), lit_i(3))).unwrap(),
            Value::Float(8.0)
        );
    }
    #[test]
    fn pow_null() {
        assert_eq!(
            run(&bin(BinaryOp::Pow, lit_null(), lit_i(1))).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn pow_invalid() {
        assert!(run(&bin(BinaryOp::Pow, lit_s("a"), lit_s("b"))).is_err());
    }
}
