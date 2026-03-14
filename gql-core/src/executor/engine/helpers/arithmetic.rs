//! Binary arithmetic and value-to-expression conversion for the query executor.

use crate::ast::Expr;
use crate::error::GqlError;
use crate::executor::filter::eval_expr;
use crate::executor::filter::Row as FilterRow;
use crate::types::Value;

/// Evaluate a binary operation directly on two already-evaluated `Value`s.
///
/// This handles all types including temporal, Node, Edge which cannot be
/// round-tripped through `value_to_expr` -> `eval_expr`.
pub(crate) fn eval_binary_values(
    op: crate::ast::BinaryOp,
    lv: Value,
    rv: Value,
    ctx: &FilterRow<'_>,
) -> Result<Value, GqlError> {
    use crate::ast::BinaryOp;
    // Handle list concatenation directly BEFORE the needs_direct check,
    // because Value::List may contain Node/Edge values that value_to_expr
    // would convert to Null.
    if matches!(op, BinaryOp::Add | BinaryOp::Concat) {
        if let (Value::List(mut l1), Value::List(l2)) = (lv.clone(), rv.clone()) {
            l1.extend(l2);
            return Ok(Value::List(l1));
        }
    }
    // For types that value_to_expr can represent losslessly, delegate to eval_expr.
    let needs_direct = matches!(
        (&lv, &rv),
        (Value::Date(_), _)
            | (_, Value::Date(_))
            | (Value::LocalTime(_), _)
            | (_, Value::LocalTime(_))
            | (Value::Time(_, _), _)
            | (_, Value::Time(_, _))
            | (Value::LocalDateTime(_), _)
            | (_, Value::LocalDateTime(_))
            | (Value::DateTime(_), _)
            | (_, Value::DateTime(_))
            | (Value::Duration(_), _)
            | (_, Value::Duration(_))
            | (Value::Node(_), _)
            | (_, Value::Node(_))
            | (Value::Edge(_), _)
            | (_, Value::Edge(_))
            | (Value::Path { .. }, _)
            | (_, Value::Path { .. })
    );
    if !needs_direct {
        let left_lit = value_to_expr(&lv);
        let right_lit = value_to_expr(&rv);
        let rebuilt = Expr::BinaryOp {
            op,
            left: Box::new(left_lit),
            right: Box::new(right_lit),
        };
        return eval_expr(&rebuilt, ctx);
    }
    // Handle comparison operators directly for temporal/graph types.
    match op {
        BinaryOp::Eq => Ok(match lv.cypher_eq(&rv) {
            Some(b) => Value::Bool(b),
            None => Value::Null,
        }),
        BinaryOp::Neq => Ok(match lv.cypher_eq(&rv) {
            Some(b) => Value::Bool(!b),
            None => Value::Null,
        }),
        BinaryOp::Lt | BinaryOp::Lte | BinaryOp::Gt | BinaryOp::Gte => {
            if lv.is_null() || rv.is_null() {
                return Ok(Value::Null);
            }
            // Cypher SS7.2 Comparability: NaN compared with a numeric yields false;
            // NaN compared with a non-numeric (cross-type) yields null.
            // This is different from ORDER BY, where cmp_values() sorts NaN last.
            let lv_is_nan = matches!(&lv, Value::Float(f) if f.is_nan());
            let rv_is_nan = matches!(&rv, Value::Float(f) if f.is_nan());
            if lv_is_nan || rv_is_nan {
                let lv_numeric = matches!(&lv, Value::Int(_) | Value::Float(_));
                let rv_numeric = matches!(&rv, Value::Int(_) | Value::Float(_));
                return Ok(if lv_numeric && rv_numeric {
                    Value::Bool(false) // NaN comparisons between numbers are always false
                } else {
                    Value::Null // cross-type with NaN
                });
            }
            // Use Cypher comparison semantics: cross-type comparisons yield null,
            // not a total-order result.  cmp_values() is for ORDER BY (sorting only).
            let ord = match crate::executor::filter::cypher_compare(&lv, &rv) {
                Ok(ord) => ord,
                Err(_) => return Ok(Value::Null),
            };
            let result = match op {
                BinaryOp::Lt => ord == std::cmp::Ordering::Less,
                BinaryOp::Lte => ord != std::cmp::Ordering::Greater,
                BinaryOp::Gt => ord == std::cmp::Ordering::Greater,
                BinaryOp::Gte => ord != std::cmp::Ordering::Less,
                _ => unreachable!(),
            };
            Ok(Value::Bool(result))
        }
        // Temporal arithmetic: temporal +/- duration, duration +/- duration, duration * int, etc.
        BinaryOp::Add | BinaryOp::Concat => {
            use crate::executor::filter::{duration_add, temporal_add_duration};
            match (&lv, &rv) {
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (
                    temporal @ (Value::Date(_)
                    | Value::LocalTime(_)
                    | Value::Time(_, _)
                    | Value::LocalDateTime(_)
                    | Value::DateTime(_)),
                    Value::Duration(dur),
                ) => temporal_add_duration(temporal, dur, false),
                (
                    Value::Duration(dur),
                    temporal @ (Value::Date(_)
                    | Value::LocalTime(_)
                    | Value::Time(_, _)
                    | Value::LocalDateTime(_)
                    | Value::DateTime(_)),
                ) => temporal_add_duration(temporal, dur, false),
                (Value::Duration(a), Value::Duration(b)) => Ok(Value::Duration(duration_add(a, b))),
                // List concatenation: handle directly to preserve Node/Edge/Path values
                // that value_to_expr would otherwise convert to Null.
                (Value::List(l1), Value::List(l2)) => {
                    let mut result = l1.clone();
                    result.extend_from_slice(l2);
                    Ok(Value::List(result))
                }
                _ => {
                    let left_lit = value_to_expr(&lv);
                    let right_lit = value_to_expr(&rv);
                    let rebuilt = Expr::BinaryOp {
                        op,
                        left: Box::new(left_lit),
                        right: Box::new(right_lit),
                    };
                    eval_expr(&rebuilt, ctx)
                }
            }
        }
        BinaryOp::Sub => {
            use crate::executor::filter::{duration_sub, temporal_add_duration};
            match (&lv, &rv) {
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (
                    temporal @ (Value::Date(_)
                    | Value::LocalTime(_)
                    | Value::Time(_, _)
                    | Value::LocalDateTime(_)
                    | Value::DateTime(_)),
                    Value::Duration(dur),
                ) => temporal_add_duration(temporal, dur, true),
                (Value::Duration(a), Value::Duration(b)) => Ok(Value::Duration(duration_sub(a, b))),
                _ => {
                    let left_lit = value_to_expr(&lv);
                    let right_lit = value_to_expr(&rv);
                    let rebuilt = Expr::BinaryOp {
                        op,
                        left: Box::new(left_lit),
                        right: Box::new(right_lit),
                    };
                    eval_expr(&rebuilt, ctx)
                }
            }
        }
        BinaryOp::Mul => {
            use crate::executor::filter::{duration_mul, duration_mul_f64};
            match (&lv, &rv) {
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (Value::Duration(d), Value::Int(n)) => Ok(Value::Duration(duration_mul(d, *n))),
                (Value::Int(n), Value::Duration(d)) => Ok(Value::Duration(duration_mul(d, *n))),
                (Value::Duration(d), Value::Float(f)) => {
                    Ok(Value::Duration(duration_mul_f64(d, *f)))
                }
                (Value::Float(f), Value::Duration(d)) => {
                    Ok(Value::Duration(duration_mul_f64(d, *f)))
                }
                _ => {
                    let left_lit = value_to_expr(&lv);
                    let right_lit = value_to_expr(&rv);
                    let rebuilt = Expr::BinaryOp {
                        op,
                        left: Box::new(left_lit),
                        right: Box::new(right_lit),
                    };
                    eval_expr(&rebuilt, ctx)
                }
            }
        }
        BinaryOp::Div => {
            use crate::executor::filter::{duration_div, duration_mul_f64};
            match (&lv, &rv) {
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (Value::Duration(d), Value::Int(n)) => duration_div(d, *n).map(Value::Duration),
                (Value::Duration(d), Value::Float(f)) => {
                    Ok(Value::Duration(duration_mul_f64(d, 1.0 / f)))
                }
                _ => {
                    let left_lit = value_to_expr(&lv);
                    let right_lit = value_to_expr(&rv);
                    let rebuilt = Expr::BinaryOp {
                        op,
                        left: Box::new(left_lit),
                        right: Box::new(right_lit),
                    };
                    eval_expr(&rebuilt, ctx)
                }
            }
        }
        // For other ops (And, Or, In, etc.) fall through to eval_expr with best-effort conversion.
        _ => {
            let left_lit = value_to_expr(&lv);
            let right_lit = value_to_expr(&rv);
            let rebuilt = Expr::BinaryOp {
                op,
                left: Box::new(left_lit),
                right: Box::new(right_lit),
            };
            eval_expr(&rebuilt, ctx)
        }
    }
}

/// Convert a `Value` back to a literal `Expr` for re-evaluation.
///
/// Used by `eval_with_storage` to handle compound expressions where sub-parts
/// have already been evaluated against storage.
pub(crate) fn value_to_expr(val: &Value) -> Expr {
    match val {
        Value::Null => Expr::Null,
        Value::Bool(b) => Expr::Bool(*b),
        Value::Int(n) => Expr::Integer(*n),
        Value::Float(f) => Expr::Float(*f),
        Value::Str(s) => Expr::Str(s.clone()),
        Value::List(items) => Expr::List(items.iter().map(value_to_expr).collect()),
        Value::Map(m) => Expr::MapLiteral(
            m.iter()
                .map(|(k, v)| (k.as_ref().into(), value_to_expr(v)))
                .collect(),
        ),
        // Node/Edge/Path/Temporal/Spatial: return as a variable with a special sentinel.
        // These should not appear in the middle of a compound expression being rebuilt.
        _ => Expr::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::BinaryOp;
    use std::collections::HashMap;

    #[test]
    fn eval_binary_values_list_concat() {
        let ctx: FilterRow<'_> = HashMap::new();
        let result = eval_binary_values(
            BinaryOp::Add,
            Value::List(vec![Value::Int(1)]),
            Value::List(vec![Value::Int(2)]),
            &ctx,
        )
        .unwrap();
        assert_eq!(result, Value::List(vec![Value::Int(1), Value::Int(2)]));
    }

    #[test]
    fn value_to_expr_roundtrip() {
        assert_eq!(value_to_expr(&Value::Null), Expr::Null);
        assert_eq!(value_to_expr(&Value::Bool(true)), Expr::Bool(true));
        assert_eq!(value_to_expr(&Value::Int(42)), Expr::Integer(42));
    }
}
