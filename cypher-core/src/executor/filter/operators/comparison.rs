//! Comparison operators: `=`, `<>`, `<`, `<=`, `>`, `>=`.
//!
//! Also hosts the cross-cutting [`cypher_compare`] total-order helper used by
//! ORDER BY and the [`safe_int_float_cmp`] helper that avoids precision loss
//! when comparing `i64` against `f64`.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use crate::executor::filter::temporal::time_to_utc_nanos;

pub(super) fn eq(lv: &Value, rv: &Value) -> Value {
    match lv.cypher_eq(rv) {
        Some(b) => Value::Bool(b),
        None => Value::Null,
    }
}

pub(super) fn neq(lv: &Value, rv: &Value) -> Value {
    match lv.cypher_eq(rv) {
        Some(b) => Value::Bool(!b),
        None => Value::Null,
    }
}

/// Helper for ordered comparisons.  `also_eq` is true for `<=` / `>=`.
///
/// Cypher semantics (openCypher §7.2 Comparability):
/// - NaN compared with a numeric value yields `false` for all ordered comparisons.
/// - NaN compared with a non-numeric value yields `null` (cross-type).
/// - All other cross-type comparisons yield `null`.
/// - This is distinct from ORDER BY, where `cmp_values()` places NaN after regular floats.
pub(super) fn compare(
    lv: &Value,
    rv: &Value,
    target: std::cmp::Ordering,
    also_eq: bool,
) -> Result<Value, GqlError> {
    if lv.is_null() || rv.is_null() {
        return Ok(Value::Null);
    }
    // NaN check: a float NaN does not compare equal to, less than, or greater than anything.
    let lv_is_nan = matches!(lv, Value::Float(f) if f.is_nan());
    let rv_is_nan = matches!(rv, Value::Float(f) if f.is_nan());
    if lv_is_nan || rv_is_nan {
        let lv_numeric = matches!(lv, Value::Int(_) | Value::Float(_));
        let rv_numeric = matches!(rv, Value::Int(_) | Value::Float(_));
        return Ok(if lv_numeric && rv_numeric {
            Value::Bool(false) // NaN <op> number and number <op> NaN are always false
        } else {
            Value::Null // cross-type
        });
    }
    match cypher_compare(lv, rv) {
        Ok(ord) => {
            let result = if also_eq {
                ord == target || ord == std::cmp::Ordering::Equal
            } else {
                ord == target
            };
            Ok(Value::Bool(result))
        }
        // Incompatible types: Cypher returns null for cross-type comparison
        Err(_) => Ok(Value::Null),
    }
}

/// Compare an i64 and f64 without precision loss for large integers.
///
/// A plain `i as f64` cast silently rounds integers beyond 2^53, which can
/// produce incorrect comparison results.  This function detects the rounding
/// case and falls back to integer-based bounds checking.
pub(crate) fn safe_int_float_cmp(i: i64, f: f64) -> Option<std::cmp::Ordering> {
    if f.is_nan() {
        return None;
    }
    let as_f = i as f64;
    if as_f as i64 == i {
        // Exact representation -- safe to compare as f64
        as_f.partial_cmp(&f)
    } else {
        // Precision loss -- compare by converting float to integer bounds
        if f > i64::MAX as f64 {
            return Some(std::cmp::Ordering::Less);
        }
        if f < i64::MIN as f64 {
            return Some(std::cmp::Ordering::Greater);
        }
        let f_trunc = f as i64;
        match i.cmp(&f_trunc) {
            std::cmp::Ordering::Equal => {
                // i == trunc(f), compare fractional part
                if f > f_trunc as f64 {
                    Some(std::cmp::Ordering::Less)
                } else if f < f_trunc as f64 {
                    Some(std::cmp::Ordering::Greater)
                } else {
                    Some(std::cmp::Ordering::Equal)
                }
            }
            other => Some(other),
        }
    }
}

/// Total order comparison for Cypher values (comparable types only).
pub(crate) fn cypher_compare(a: &Value, b: &Value) -> Result<std::cmp::Ordering, GqlError> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(x.cmp(y)),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).ok_or_else(|| {
            GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "comparable Float".to_string(),
                actual: "NaN".to_string(),
            })
        }),
        (Value::Int(x), Value::Float(y)) => {
            if y.is_nan() {
                return Ok(std::cmp::Ordering::Less);
            }
            safe_int_float_cmp(*x, *y).ok_or_else(|| {
                GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "comparable Float".to_string(),
                    actual: "NaN".to_string(),
                })
            })
        }
        (Value::Float(x), Value::Int(y)) => {
            if x.is_nan() {
                return Ok(std::cmp::Ordering::Greater);
            }
            safe_int_float_cmp(*y, *x)
                .map(|ord| ord.reverse())
                .ok_or_else(|| {
                    GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "comparable Float".to_string(),
                        actual: "NaN".to_string(),
                    })
                })
        }
        (Value::Str(x), Value::Str(y)) => Ok(x.as_ref().cmp(y.as_ref())),
        (Value::Bool(x), Value::Bool(y)) => Ok(x.cmp(y)),
        // Temporal comparisons
        (Value::Date(x), Value::Date(y)) => Ok(x.cmp(y)),
        (Value::LocalTime(x), Value::LocalTime(y)) => Ok(x.cmp(y)),
        // Zoned time: compare by UTC-normalized instant
        (Value::Time(t1, o1), Value::Time(t2, o2)) => {
            let utc1 = time_to_utc_nanos(t1, *o1);
            let utc2 = time_to_utc_nanos(t2, *o2);
            Ok(utc1.cmp(&utc2))
        }
        (Value::LocalDateTime(x), Value::LocalDateTime(y)) => Ok(x.cmp(y)),
        (Value::DateTime(x), Value::DateTime(y)) => Ok(x.cmp(y)),
        // List comparison: element-by-element
        (Value::List(x), Value::List(y)) => {
            for (a, b) in x.iter().zip(y.iter()) {
                match cypher_compare(a, b) {
                    Ok(std::cmp::Ordering::Equal) => continue,
                    Ok(ord) => return Ok(ord),
                    Err(e) => return Err(e),
                }
            }
            Ok(x.len().cmp(&y.len()))
        }
        (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
            op: "</>".to_string(),
            left: l.type_name().to_string(),
            right: r.type_name().to_string(),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_utils::{bin, lit_f, lit_i, lit_null, lit_s, run};
    use super::*;
    use crate::ast::BinaryOp;

    #[test]
    fn eq_neq() {
        assert_eq!(
            run(&bin(BinaryOp::Eq, lit_i(1), lit_i(1))).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            run(&bin(BinaryOp::Neq, lit_i(1), lit_i(2))).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            run(&bin(BinaryOp::Eq, lit_null(), lit_i(1))).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn lt_lte_gt_gte() {
        assert_eq!(
            run(&bin(BinaryOp::Lt, lit_i(1), lit_i(2))).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            run(&bin(BinaryOp::Lte, lit_i(2), lit_i(2))).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            run(&bin(BinaryOp::Gt, lit_i(3), lit_i(2))).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            run(&bin(BinaryOp::Gte, lit_i(2), lit_i(2))).unwrap(),
            Value::Bool(true)
        );
    }
    #[test]
    fn cmp_with_null() {
        assert_eq!(
            run(&bin(BinaryOp::Lt, lit_null(), lit_i(1))).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn cmp_with_nan() {
        assert_eq!(
            run(&bin(BinaryOp::Lt, lit_f(f64::NAN), lit_i(1))).unwrap(),
            Value::Bool(false)
        );
        // NaN vs non-numeric → null
        assert_eq!(
            run(&bin(BinaryOp::Lt, lit_f(f64::NAN), lit_s("a"))).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn cmp_cross_type_returns_null() {
        assert_eq!(
            run(&bin(BinaryOp::Lt, lit_i(1), lit_s("a"))).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn safe_int_float_basic() {
        use std::cmp::Ordering;
        assert_eq!(safe_int_float_cmp(1, 2.0), Some(Ordering::Less));
        assert_eq!(safe_int_float_cmp(2, 2.0), Some(Ordering::Equal));
        assert_eq!(safe_int_float_cmp(3, 2.0), Some(Ordering::Greater));
        assert_eq!(safe_int_float_cmp(1, f64::NAN), None);
    }
    #[test]
    fn safe_int_float_large_int() {
        use std::cmp::Ordering;
        // 2^60 > 2^53, exceeds float precision when cast.
        let big = 1_i64 << 60;
        // Float much greater than i64::MAX
        assert_eq!(safe_int_float_cmp(big, f64::INFINITY), Some(Ordering::Less));
        assert_eq!(
            safe_int_float_cmp(big, f64::NEG_INFINITY),
            Some(Ordering::Greater)
        );
        // Float well above i64::MAX is still > big
        assert_eq!(safe_int_float_cmp(big, 1.0e30), Some(Ordering::Less));
        // Float well below i64::MIN
        assert_eq!(safe_int_float_cmp(big, -1.0e30), Some(Ordering::Greater));
    }
    #[test]
    fn cypher_compare_strs_bools_dates() {
        use std::cmp::Ordering;
        assert_eq!(
            cypher_compare(&Value::Str("a".into()), &Value::Str("b".into())).unwrap(),
            Ordering::Less
        );
        assert_eq!(
            cypher_compare(&Value::Bool(false), &Value::Bool(true)).unwrap(),
            Ordering::Less
        );
        let d1 = Value::Date(jiff::civil::Date::new(2024, 1, 1).unwrap());
        let d2 = Value::Date(jiff::civil::Date::new(2024, 2, 1).unwrap());
        assert_eq!(cypher_compare(&d1, &d2).unwrap(), Ordering::Less);
    }
    #[test]
    fn cypher_compare_lists() {
        use std::cmp::Ordering;
        let a = Value::List(vec![Value::Int(1), Value::Int(2)]);
        let b = Value::List(vec![Value::Int(1), Value::Int(3)]);
        assert_eq!(cypher_compare(&a, &b).unwrap(), Ordering::Less);
        let c = Value::List(vec![Value::Int(1)]);
        let d = Value::List(vec![Value::Int(1), Value::Int(2)]);
        assert_eq!(cypher_compare(&c, &d).unwrap(), Ordering::Less);
    }
    #[test]
    fn cypher_compare_int_float_with_nan() {
        use std::cmp::Ordering;
        assert_eq!(
            cypher_compare(&Value::Int(1), &Value::Float(f64::NAN)).unwrap(),
            Ordering::Less
        );
        assert_eq!(
            cypher_compare(&Value::Float(f64::NAN), &Value::Int(1)).unwrap(),
            Ordering::Greater
        );
    }
    #[test]
    fn cypher_compare_incompatible_errs() {
        assert!(cypher_compare(&Value::Int(1), &Value::Str("a".into())).is_err());
    }
}
