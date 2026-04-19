//! Binary and unary operator evaluation, comparison logic.

use crate::ast::{BinaryOp, UnaryOp};
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::temporal::{
    duration_add, duration_div, duration_mul, duration_mul_f64, duration_sub,
    temporal_add_duration, time_to_utc_nanos,
};
use super::{eval_expr, Row, REGEX_CACHE};

// ── Unary operator evaluation ─────────────────────────────────────────────────

pub(super) fn eval_unary<'r>(
    op: UnaryOp,
    expr: &crate::ast::Expr,
    row: &Row<'r>,
) -> Result<Value, GqlError> {
    let val = eval_expr(expr, row)?;
    match op {
        UnaryOp::Neg => match val {
            Value::Null => Ok(Value::Null),
            Value::Int(n) => Ok(Value::Int(n.checked_neg().ok_or_else(|| {
                GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "integer arithmetic without overflow".to_string(),
                    actual: "integer overflow".to_string(),
                })
            })?)),
            Value::Float(f) => Ok(Value::Float(-f)),
            other => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "-".to_string(),
                left: other.type_name().to_string(),
                right: "".to_string(),
            })),
        },
        UnaryOp::Not => match val {
            Value::Null => Ok(Value::Null),
            Value::Bool(b) => Ok(Value::Bool(!b)),
            other => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "NOT".to_string(),
                left: other.type_name().to_string(),
                right: "".to_string(),
            })),
        },
    }
}

// ── Binary operator evaluation ────────────────────────────────────────────────

pub(super) fn eval_binary<'r>(
    op: BinaryOp,
    left: &crate::ast::Expr,
    right: &crate::ast::Expr,
    row: &Row<'r>,
) -> Result<Value, GqlError> {
    // Short-circuit AND / OR before evaluating both sides.
    match op {
        BinaryOp::And => {
            let lv = eval_expr(left, row)?;
            if lv == Value::Bool(false) {
                return Ok(Value::Bool(false));
            }
            let rv = eval_expr(right, row)?;
            return match (lv, rv) {
                (Value::Null, Value::Bool(false)) | (Value::Bool(false), Value::Null) => {
                    Ok(Value::Bool(false))
                }
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a && b)),
                (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                    op: "AND".to_string(),
                    left: l.type_name().to_string(),
                    right: r.type_name().to_string(),
                })),
            };
        }
        BinaryOp::Or => {
            let lv = eval_expr(left, row)?;
            if lv == Value::Bool(true) {
                return Ok(Value::Bool(true));
            }
            let rv = eval_expr(right, row)?;
            return match (lv, rv) {
                (Value::Null, Value::Bool(true)) | (Value::Bool(true), Value::Null) => {
                    Ok(Value::Bool(true))
                }
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a || b)),
                (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                    op: "OR".to_string(),
                    left: l.type_name().to_string(),
                    right: r.type_name().to_string(),
                })),
            };
        }
        _ => {}
    }

    let lv = eval_expr(left, row)?;
    let rv = eval_expr(right, row)?;

    match op {
        BinaryOp::And | BinaryOp::Or => unreachable!("handled above"),

        // ── Xor ──────────────────────────────────────────────────────────────
        BinaryOp::Xor => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a ^ b)),
            (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "XOR".to_string(),
                left: l.type_name().to_string(),
                right: r.type_name().to_string(),
            })),
        },

        // ── Arithmetic ────────────────────────────────────────────────────────
        BinaryOp::Add | BinaryOp::Concat => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Int(a), Value::Int(b)) => {
                Ok(Value::Int(a.checked_add(b).ok_or_else(|| {
                    GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "integer arithmetic without overflow".to_string(),
                        actual: "integer overflow".to_string(),
                    })
                })?))
            }
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
        },

        BinaryOp::Sub => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Int(a), Value::Int(b)) => {
                Ok(Value::Int(a.checked_sub(b).ok_or_else(|| {
                    GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "integer arithmetic without overflow".to_string(),
                        actual: "integer overflow".to_string(),
                    })
                })?))
            }
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
        },

        BinaryOp::Mul => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Int(a), Value::Int(b)) => {
                Ok(Value::Int(a.checked_mul(b).ok_or_else(|| {
                    GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "integer arithmetic without overflow".to_string(),
                        actual: "integer overflow".to_string(),
                    })
                })?))
            }
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
        },

        BinaryOp::Div => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Int(_), Value::Int(0)) => {
                Err(GqlError::Execution(ExecutionError::DivisionByZero))
            }
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a / b)),
            // Float division by zero follows IEEE 754: returns Inf, -Inf, or NaN.
            // The TCK expects this behavior (e.g. 0.0/0.0 → NaN).
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a / b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 / b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a / b as f64)),
            // Duration / Integer
            (Value::Duration(d), Value::Int(n)) => duration_div(&d, n).map(Value::Duration),
            // Duration / Float: duration / f = duration * (1/f)
            (Value::Duration(d), Value::Float(f)) => {
                Ok(Value::Duration(duration_mul_f64(&d, 1.0 / f)))
            }
            (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "/".to_string(),
                left: l.type_name().to_string(),
                right: r.type_name().to_string(),
            })),
        },

        BinaryOp::Mod => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Int(_), Value::Int(0)) => {
                Err(GqlError::Execution(ExecutionError::DivisionByZero))
            }
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
        },

        BinaryOp::Pow => match (lv, rv) {
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
        },

        // ── Comparison ────────────────────────────────────────────────────────
        BinaryOp::Eq => Ok(match lv.cypher_eq(&rv) {
            Some(b) => Value::Bool(b),
            None => Value::Null,
        }),
        BinaryOp::Neq => Ok(match lv.cypher_eq(&rv) {
            Some(b) => Value::Bool(!b),
            None => Value::Null,
        }),
        BinaryOp::Lt => compare_values(&lv, &rv, std::cmp::Ordering::Less, false),
        BinaryOp::Lte => compare_values(&lv, &rv, std::cmp::Ordering::Less, true),
        BinaryOp::Gt => compare_values(&lv, &rv, std::cmp::Ordering::Greater, false),
        BinaryOp::Gte => compare_values(&lv, &rv, std::cmp::Ordering::Greater, true),

        // ── String predicates ─────────────────────────────────────────────────
        BinaryOp::StartsWith => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Str(s), Value::Str(prefix)) => Ok(Value::Bool(s.starts_with(prefix.as_ref()))),
            // Type mismatch in string predicates returns null in Cypher
            (_, _) => Ok(Value::Null),
        },
        BinaryOp::EndsWith => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Str(s), Value::Str(suffix)) => Ok(Value::Bool(s.ends_with(suffix.as_ref()))),
            (_, _) => Ok(Value::Null),
        },
        BinaryOp::Contains => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Str(s), Value::Str(needle)) => Ok(Value::Bool(s.contains(needle.as_ref()))),
            (_, _) => Ok(Value::Null),
        },
        BinaryOp::RegexMatch => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Str(s), Value::Str(pattern)) => {
                let max_pattern_len = crate::config::global().max_regex_pattern_len;
                if pattern.len() > max_pattern_len {
                    return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: format!("regex pattern <= {max_pattern_len} bytes"),
                        actual: format!("{} bytes", pattern.len()),
                    }));
                }
                // Cypher's =~ operator requires the pattern to match the ENTIRE
                // string (implicit ^..$ anchoring), but regex::is_match() succeeds
                // on substring matches.  Wrap with ^(?:...)$ so the compiled regex
                // enforces a full-string match.
                let anchored = format!("^(?:{})$", pattern);
                let matched = REGEX_CACHE.with(|cache| -> Result<bool, GqlError> {
                    let mut map = cache.borrow_mut();
                    if !map.contains_key(anchored.as_str()) {
                        let re = regex::Regex::new(&anchored).map_err(|e| {
                            GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "valid regex pattern".to_string(),
                                actual: e.to_string(),
                            })
                        })?;
                        if map.len() >= crate::config::global().regex_cache_capacity {
                            // Evict the oldest half instead of clearing everything.
                            // This avoids a "cache cliff" where all compiled regexes
                            // are dropped simultaneously, causing a spike of recompilation.
                            let half = map.len() / 2;
                            map.drain(..half);
                        }
                        map.insert(anchored.to_string(), re);
                    }
                    Ok(map[anchored.as_str()].is_match(s.as_ref()))
                })?;
                Ok(Value::Bool(matched))
            }
            (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "=~".to_string(),
                left: l.type_name().to_string(),
                right: r.type_name().to_string(),
            })),
        },

        // ── IN operator ───────────────────────────────────────────────────────
        BinaryOp::In => match rv {
            Value::Null => Ok(Value::Null),
            Value::List(items) => {
                if lv.is_null() {
                    // null IN [] → false; null IN [any non-empty list] → null
                    // Because null == anything is null, so membership is always unknown
                    // for non-empty lists.
                    if items.is_empty() {
                        Ok(Value::Bool(false))
                    } else {
                        Ok(Value::Null)
                    }
                } else {
                    // x IN list: true if any element equals x; null if null present and not found.
                    let mut found_null = false;
                    for item in &items {
                        match lv.cypher_eq(item) {
                            Some(true) => return Ok(Value::Bool(true)),
                            None => found_null = true,
                            Some(false) => {}
                        }
                    }
                    if found_null {
                        Ok(Value::Null)
                    } else {
                        Ok(Value::Bool(false))
                    }
                }
            }
            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "List".to_string(),
                actual: other.type_name().to_string(),
            })),
        },
    }
}

/// Helper for ordered comparisons.  `also_eq` is true for `<=` / `>=`.
///
/// Cypher semantics (openCypher §7.2 Comparability):
/// - NaN compared with a numeric value yields `false` for all ordered comparisons.
/// - NaN compared with a non-numeric value yields `null` (cross-type).
/// - All other cross-type comparisons yield `null`.
/// - This is distinct from ORDER BY, where `cmp_values()` places NaN after regular floats.
fn compare_values(
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
    use super::*;
    use crate::ast::Expr;
    use std::collections::HashMap;

    fn empty_row<'a>() -> Row<'a> {
        HashMap::new()
    }
    fn lit_i(n: i64) -> Expr {
        Expr::Integer(n)
    }
    fn lit_f(n: f64) -> Expr {
        Expr::Float(n)
    }
    fn lit_s(s: &str) -> Expr {
        Expr::Str(s.into())
    }
    fn lit_b(b: bool) -> Expr {
        Expr::Bool(b)
    }
    fn lit_null() -> Expr {
        Expr::Null
    }
    fn bin(op: BinaryOp, l: Expr, r: Expr) -> Expr {
        Expr::BinaryOp {
            op,
            left: Box::new(l),
            right: Box::new(r),
        }
    }
    fn una(op: UnaryOp, e: Expr) -> Expr {
        Expr::UnaryOp {
            op,
            expr: Box::new(e),
        }
    }
    fn run(e: &Expr) -> Result<Value, GqlError> {
        eval_expr(e, &empty_row())
    }

    // ── Unary ─────────────────────────────────────────────────────────
    #[test]
    fn neg_int() {
        assert_eq!(run(&una(UnaryOp::Neg, lit_i(5))).unwrap(), Value::Int(-5));
    }
    #[test]
    fn neg_float() {
        assert_eq!(
            run(&una(UnaryOp::Neg, lit_f(1.5))).unwrap(),
            Value::Float(-1.5)
        );
    }
    #[test]
    fn neg_null() {
        assert_eq!(run(&una(UnaryOp::Neg, lit_null())).unwrap(), Value::Null);
    }
    #[test]
    fn neg_overflow() {
        assert!(run(&una(UnaryOp::Neg, lit_i(i64::MIN))).is_err());
    }
    #[test]
    fn neg_string_errs() {
        assert!(run(&una(UnaryOp::Neg, lit_s("a"))).is_err());
    }
    #[test]
    fn not_bool() {
        assert_eq!(
            run(&una(UnaryOp::Not, lit_b(true))).unwrap(),
            Value::Bool(false)
        );
    }
    #[test]
    fn not_null() {
        assert_eq!(run(&una(UnaryOp::Not, lit_null())).unwrap(), Value::Null);
    }
    #[test]
    fn not_int_errs() {
        assert!(run(&una(UnaryOp::Not, lit_i(0))).is_err());
    }

    // ── AND / OR / XOR ───────────────────────────────────────────────
    #[test]
    fn and_true_false() {
        assert_eq!(
            run(&bin(BinaryOp::And, lit_b(true), lit_b(false))).unwrap(),
            Value::Bool(false)
        );
    }
    #[test]
    fn and_short_circuit_false() {
        assert_eq!(
            run(&bin(BinaryOp::And, lit_b(false), lit_i(99))).unwrap(),
            Value::Bool(false)
        );
    }
    #[test]
    fn and_null_false() {
        assert_eq!(
            run(&bin(BinaryOp::And, lit_null(), lit_b(false))).unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            run(&bin(BinaryOp::And, lit_b(false), lit_null())).unwrap(),
            Value::Bool(false)
        );
    }
    #[test]
    fn and_null_true() {
        assert_eq!(
            run(&bin(BinaryOp::And, lit_null(), lit_b(true))).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn and_type_err() {
        assert!(run(&bin(BinaryOp::And, lit_i(1), lit_b(true))).is_err());
    }
    #[test]
    fn or_short_circuit_true() {
        assert_eq!(
            run(&bin(BinaryOp::Or, lit_b(true), lit_i(99))).unwrap(),
            Value::Bool(true)
        );
    }
    #[test]
    fn or_null_true() {
        assert_eq!(
            run(&bin(BinaryOp::Or, lit_null(), lit_b(true))).unwrap(),
            Value::Bool(true)
        );
    }
    #[test]
    fn or_null_null() {
        assert_eq!(
            run(&bin(BinaryOp::Or, lit_null(), lit_b(false))).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn or_type_err() {
        assert!(run(&bin(BinaryOp::Or, lit_b(false), lit_i(1))).is_err());
    }
    #[test]
    fn xor_basic() {
        assert_eq!(
            run(&bin(BinaryOp::Xor, lit_b(true), lit_b(false))).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            run(&bin(BinaryOp::Xor, lit_b(true), lit_b(true))).unwrap(),
            Value::Bool(false)
        );
    }
    #[test]
    fn xor_null() {
        assert_eq!(
            run(&bin(BinaryOp::Xor, lit_null(), lit_b(true))).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn xor_type_err() {
        assert!(run(&bin(BinaryOp::Xor, lit_i(1), lit_i(2))).is_err());
    }

    // ── Arithmetic ──────────────────────────────────────────────────────
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
        assert!(run(&bin(BinaryOp::Add, lit_b(true), lit_b(false))).is_err());
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

    // ── Comparison ──────────────────────────────────────────────────────
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

    // ── String predicates ──────────────────────────────────────────────
    #[test]
    fn starts_with_basic() {
        assert_eq!(
            run(&bin(BinaryOp::StartsWith, lit_s("abc"), lit_s("ab"))).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            run(&bin(BinaryOp::StartsWith, lit_null(), lit_s("a"))).unwrap(),
            Value::Null
        );
        assert_eq!(
            run(&bin(BinaryOp::StartsWith, lit_i(1), lit_s("a"))).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn ends_with_basic() {
        assert_eq!(
            run(&bin(BinaryOp::EndsWith, lit_s("abc"), lit_s("bc"))).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            run(&bin(BinaryOp::EndsWith, lit_null(), lit_s("a"))).unwrap(),
            Value::Null
        );
        assert_eq!(
            run(&bin(BinaryOp::EndsWith, lit_i(1), lit_s("a"))).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn contains_basic() {
        assert_eq!(
            run(&bin(BinaryOp::Contains, lit_s("hello world"), lit_s("o w"))).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            run(&bin(BinaryOp::Contains, lit_null(), lit_s("a"))).unwrap(),
            Value::Null
        );
        assert_eq!(
            run(&bin(BinaryOp::Contains, lit_i(1), lit_s("a"))).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn regex_match() {
        assert_eq!(
            run(&bin(BinaryOp::RegexMatch, lit_s("hello"), lit_s("h.*o"))).unwrap(),
            Value::Bool(true)
        );
        // anchored: must match entire string
        assert_eq!(
            run(&bin(
                BinaryOp::RegexMatch,
                lit_s("hello world"),
                lit_s("h.*o")
            ))
            .unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            run(&bin(BinaryOp::RegexMatch, lit_null(), lit_s("a"))).unwrap(),
            Value::Null
        );
        assert!(run(&bin(BinaryOp::RegexMatch, lit_i(1), lit_s("a"))).is_err());
        // bad pattern
        assert!(run(&bin(BinaryOp::RegexMatch, lit_s("a"), lit_s("("))).is_err());
    }

    // ── IN operator ─────────────────────────────────────────────────────
    #[test]
    fn in_basic_true() {
        let l = Expr::List(vec![lit_i(1), lit_i(2), lit_i(3)]);
        assert_eq!(
            run(&bin(BinaryOp::In, lit_i(2), l)).unwrap(),
            Value::Bool(true)
        );
    }
    #[test]
    fn in_basic_false() {
        let l = Expr::List(vec![lit_i(1), lit_i(2)]);
        assert_eq!(
            run(&bin(BinaryOp::In, lit_i(99), l)).unwrap(),
            Value::Bool(false)
        );
    }
    #[test]
    fn in_with_null_in_list() {
        let l = Expr::List(vec![lit_i(1), lit_null()]);
        assert_eq!(run(&bin(BinaryOp::In, lit_i(99), l)).unwrap(), Value::Null);
    }
    #[test]
    fn in_null_value() {
        let l_empty = Expr::List(vec![]);
        assert_eq!(
            run(&bin(BinaryOp::In, lit_null(), l_empty)).unwrap(),
            Value::Bool(false)
        );
        let l_nonempty = Expr::List(vec![lit_i(1)]);
        assert_eq!(
            run(&bin(BinaryOp::In, lit_null(), l_nonempty)).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn in_rhs_null() {
        assert_eq!(
            run(&bin(BinaryOp::In, lit_i(1), lit_null())).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn in_rhs_not_list_errs() {
        assert!(run(&bin(BinaryOp::In, lit_i(1), lit_i(1))).is_err());
    }

    // ── safe_int_float_cmp / cypher_compare ────────────────────────────
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
