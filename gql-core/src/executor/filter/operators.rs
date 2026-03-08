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
                        if map.len() >= 512 {
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
fn safe_int_float_cmp(i: i64, f: f64) -> Option<std::cmp::Ordering> {
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
