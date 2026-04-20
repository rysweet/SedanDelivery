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
        // For other ops (And, Or, etc.) fall through to eval_expr with best-effort conversion.
        // BinaryOp::In must be handled directly because value_to_expr loses
        // Node/Edge values (converts them to Null), breaking membership tests
        // against lists of graph elements.
        BinaryOp::In => match rv {
            Value::Null => Ok(Value::Null),
            Value::List(items) => {
                if lv.is_null() {
                    if items.is_empty() {
                        Ok(Value::Bool(false))
                    } else {
                        Ok(Value::Null)
                    }
                } else {
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
        },
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

    // ── Extended coverage ────────────────────────────────────────────────

    use crate::types::CypherDuration;

    fn ctx() -> FilterRow<'static> {
        HashMap::new()
    }

    #[test]
    fn value_to_expr_covers_remaining_variants() {
        match value_to_expr(&Value::Float(1.5)) {
            Expr::Float(f) => assert_eq!(f, 1.5),
            other => panic!("expected Float, got {other:?}"),
        }
        match value_to_expr(&Value::Str("hi".into())) {
            Expr::Str(s) => assert_eq!(&*s, "hi"),
            other => panic!("expected Str, got {other:?}"),
        }
        // List recurses into items.
        match value_to_expr(&Value::List(vec![Value::Int(1), Value::Bool(false)])) {
            Expr::List(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0], Expr::Integer(1));
                assert_eq!(items[1], Expr::Bool(false));
            }
            other => panic!("expected List, got {other:?}"),
        }
        // Map recurses into values and converts keys.
        let mut m = indexmap::IndexMap::new();
        m.insert("k".into(), Value::Int(5));
        match value_to_expr(&Value::Map(m)) {
            Expr::MapLiteral(entries) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(&*entries[0].0, "k");
                assert_eq!(entries[0].1, Expr::Integer(5));
            }
            other => panic!("expected MapLiteral, got {other:?}"),
        }
        // Other (e.g. Duration) collapses to Null.
        let dur = Value::Duration(CypherDuration {
            months: 0,
            days: 0,
            nanos: 0,
        });
        assert_eq!(value_to_expr(&dur), Expr::Null);
    }

    #[test]
    fn eq_neq_with_nan_yields_false_or_null_per_cypher() {
        let nan = Value::Float(f64::NAN);
        // Eq with NaN vs Int → cypher_eq returns Some(false) → Bool(false).
        let r = eval_binary_values(crate::ast::BinaryOp::Eq, nan.clone(), Value::Int(1), &ctx())
            .unwrap();
        assert_eq!(r, Value::Bool(false));
        let r = eval_binary_values(
            crate::ast::BinaryOp::Neq,
            nan.clone(),
            Value::Int(1),
            &ctx(),
        )
        .unwrap();
        assert_eq!(r, Value::Bool(true));
    }

    #[test]
    fn comparison_with_null_is_null() {
        for op in [
            crate::ast::BinaryOp::Lt,
            crate::ast::BinaryOp::Lte,
            crate::ast::BinaryOp::Gt,
            crate::ast::BinaryOp::Gte,
        ] {
            let r = eval_binary_values(op, Value::Null, Value::Int(1), &ctx()).unwrap();
            assert_eq!(r, Value::Null);
            let r = eval_binary_values(op, Value::Int(1), Value::Null, &ctx()).unwrap();
            assert_eq!(r, Value::Null);
        }
    }

    #[test]
    fn comparison_nan_numeric_is_false() {
        let nan = Value::Float(f64::NAN);
        let r = eval_binary_values(crate::ast::BinaryOp::Lt, nan.clone(), Value::Int(1), &ctx())
            .unwrap();
        assert_eq!(r, Value::Bool(false));
        let r = eval_binary_values(
            crate::ast::BinaryOp::Gte,
            Value::Float(2.0),
            nan.clone(),
            &ctx(),
        )
        .unwrap();
        assert_eq!(r, Value::Bool(false));
    }

    #[test]
    fn comparison_nan_with_string_is_null() {
        let nan = Value::Float(f64::NAN);
        let r = eval_binary_values(
            crate::ast::BinaryOp::Lt,
            nan,
            Value::Str("x".into()),
            &ctx(),
        )
        .unwrap();
        assert_eq!(r, Value::Null);
    }

    #[test]
    fn integer_compare_lt_lte_gt_gte() {
        let r = eval_binary_values(
            crate::ast::BinaryOp::Lt,
            Value::Int(1),
            Value::Int(2),
            &ctx(),
        )
        .unwrap();
        assert_eq!(r, Value::Bool(true));
        let r = eval_binary_values(
            crate::ast::BinaryOp::Lte,
            Value::Int(2),
            Value::Int(2),
            &ctx(),
        )
        .unwrap();
        assert_eq!(r, Value::Bool(true));
        let r = eval_binary_values(
            crate::ast::BinaryOp::Gt,
            Value::Int(3),
            Value::Int(2),
            &ctx(),
        )
        .unwrap();
        assert_eq!(r, Value::Bool(true));
        let r = eval_binary_values(
            crate::ast::BinaryOp::Gte,
            Value::Int(2),
            Value::Int(3),
            &ctx(),
        )
        .unwrap();
        assert_eq!(r, Value::Bool(false));
    }

    #[test]
    fn add_with_null_propagates_null() {
        let r = eval_binary_values(
            crate::ast::BinaryOp::Add,
            Value::Null,
            Value::Int(1),
            &ctx(),
        )
        .unwrap();
        assert_eq!(r, Value::Null);
    }

    #[test]
    fn duration_add_and_sub_durations() {
        let a = Value::Duration(CypherDuration {
            months: 0,
            days: 0,
            nanos: 1_000_000_000,
        });
        let b = Value::Duration(CypherDuration {
            months: 0,
            days: 0,
            nanos: 2_000_000_000,
        });
        let r =
            eval_binary_values(crate::ast::BinaryOp::Add, a.clone(), b.clone(), &ctx()).unwrap();
        if let Value::Duration(d) = r {
            assert_eq!(d.nanos, 3_000_000_000);
        } else {
            panic!("expected Duration");
        }
        let r = eval_binary_values(crate::ast::BinaryOp::Sub, b, a, &ctx()).unwrap();
        if let Value::Duration(d) = r {
            assert_eq!(d.nanos, 1_000_000_000);
        } else {
            panic!("expected Duration");
        }
    }

    #[test]
    fn duration_mul_int_and_float_either_side() {
        let dur = Value::Duration(CypherDuration {
            months: 0,
            days: 0,
            nanos: 1_000_000_000,
        });
        // dur * int
        let r = eval_binary_values(
            crate::ast::BinaryOp::Mul,
            dur.clone(),
            Value::Int(3),
            &ctx(),
        )
        .unwrap();
        if let Value::Duration(d) = r {
            assert_eq!(d.nanos, 3_000_000_000);
        } else {
            panic!()
        }
        // int * dur
        let r = eval_binary_values(
            crate::ast::BinaryOp::Mul,
            Value::Int(2),
            dur.clone(),
            &ctx(),
        )
        .unwrap();
        if let Value::Duration(d) = r {
            assert_eq!(d.nanos, 2_000_000_000);
        } else {
            panic!()
        }
        // dur * float
        let r = eval_binary_values(
            crate::ast::BinaryOp::Mul,
            dur.clone(),
            Value::Float(0.5),
            &ctx(),
        )
        .unwrap();
        if let Value::Duration(d) = r {
            assert_eq!(d.nanos, 500_000_000);
        } else {
            panic!()
        }
        // float * dur
        let r =
            eval_binary_values(crate::ast::BinaryOp::Mul, Value::Float(2.0), dur, &ctx()).unwrap();
        if let Value::Duration(d) = r {
            assert_eq!(d.nanos, 2_000_000_000);
        } else {
            panic!()
        }
    }

    #[test]
    fn duration_div_and_null_propagation() {
        let dur = Value::Duration(CypherDuration {
            months: 0,
            days: 0,
            nanos: 6_000_000_000,
        });
        let r = eval_binary_values(
            crate::ast::BinaryOp::Div,
            dur.clone(),
            Value::Int(2),
            &ctx(),
        )
        .unwrap();
        if let Value::Duration(d) = r {
            assert_eq!(d.nanos, 3_000_000_000);
        } else {
            panic!()
        }
        let r = eval_binary_values(
            crate::ast::BinaryOp::Div,
            dur.clone(),
            Value::Float(2.0),
            &ctx(),
        )
        .unwrap();
        if let Value::Duration(d) = r {
            assert_eq!(d.nanos, 3_000_000_000);
        } else {
            panic!()
        }
        let r = eval_binary_values(
            crate::ast::BinaryOp::Div,
            Value::Null,
            Value::Int(1),
            &ctx(),
        )
        .unwrap();
        assert_eq!(r, Value::Null);
        let r = eval_binary_values(
            crate::ast::BinaryOp::Mul,
            Value::Int(1),
            Value::Null,
            &ctx(),
        )
        .unwrap();
        assert_eq!(r, Value::Null);
        let r = eval_binary_values(
            crate::ast::BinaryOp::Sub,
            Value::Null,
            Value::Int(1),
            &ctx(),
        )
        .unwrap();
        assert_eq!(r, Value::Null);
    }

    #[test]
    fn in_operator_membership_and_null_handling() {
        // present → true
        let r = eval_binary_values(
            crate::ast::BinaryOp::In,
            Value::Int(2),
            Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
            &ctx(),
        )
        .unwrap();
        assert_eq!(r, Value::Bool(true));
        // absent + no nulls → false
        let r = eval_binary_values(
            crate::ast::BinaryOp::In,
            Value::Int(9),
            Value::List(vec![Value::Int(1), Value::Int(2)]),
            &ctx(),
        )
        .unwrap();
        assert_eq!(r, Value::Bool(false));
        // absent + has null → null
        let r = eval_binary_values(
            crate::ast::BinaryOp::In,
            Value::Int(9),
            Value::List(vec![Value::Int(1), Value::Null]),
            &ctx(),
        )
        .unwrap();
        assert_eq!(r, Value::Null);
        // null lhs + empty list → false
        let r = eval_binary_values(
            crate::ast::BinaryOp::In,
            Value::Null,
            Value::List(vec![]),
            &ctx(),
        )
        .unwrap();
        assert_eq!(r, Value::Bool(false));
        // null lhs + non-empty list → null
        let r = eval_binary_values(
            crate::ast::BinaryOp::In,
            Value::Null,
            Value::List(vec![Value::Int(1)]),
            &ctx(),
        )
        .unwrap();
        assert_eq!(r, Value::Null);
        // null rhs → null
        let r = eval_binary_values(crate::ast::BinaryOp::In, Value::Int(1), Value::Null, &ctx())
            .unwrap();
        assert_eq!(r, Value::Null);
    }
}
