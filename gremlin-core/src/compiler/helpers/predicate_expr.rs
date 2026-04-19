//! Lowering of `Predicate` (P.eq/P.gt/etc.) into AST expressions.
//!
//! Extracted from `helpers.rs` as part of the helpers/ submodule split.
//! All items are `pub(in crate::compiler)` and behave identically to their pre-split
//! definitions.

use cypher_core::ast::{BinaryOp, Expr, Identifier};
use smol_str::SmolStr;

use crate::bytecode::Predicate;
use crate::types::GremlinValue;

use super::*;

/// Extract predicate operator name and value expression for all()/any()/none() steps.
pub(in crate::compiler) fn predicate_to_args(pred: &Predicate) -> (Box<str>, Expr) {
    match pred {
        Predicate::Eq(v) => ("eq".into(), gremlin_value_to_expr(v)),
        Predicate::Neq(v) => ("neq".into(), gremlin_value_to_expr(v)),
        Predicate::Gt(v) => ("gt".into(), gremlin_value_to_expr(v)),
        Predicate::Gte(v) => ("gte".into(), gremlin_value_to_expr(v)),
        Predicate::Lt(v) => ("lt".into(), gremlin_value_to_expr(v)),
        Predicate::Lte(v) => ("lte".into(), gremlin_value_to_expr(v)),
        Predicate::Within(vals) => (
            "within".into(),
            Expr::List(vals.iter().map(gremlin_value_to_expr).collect()),
        ),
        Predicate::Without(vals) => (
            "without".into(),
            Expr::List(vals.iter().map(gremlin_value_to_expr).collect()),
        ),
        Predicate::Between(low, high) => (
            "within".into(),
            Expr::List(vec![
                gremlin_value_to_expr(low),
                gremlin_value_to_expr(high),
            ]),
        ),
        Predicate::Inside(low, high) => (
            "within".into(),
            Expr::List(vec![
                gremlin_value_to_expr(low),
                gremlin_value_to_expr(high),
            ]),
        ),
        _ => ("neq".into(), Expr::Null),
    }
}

/// Convert a `Predicate` to an `Expr`.
pub(in crate::compiler) fn predicate_to_expr(pred: &Predicate, alias: &Identifier) -> Expr {
    let var = Expr::Variable(alias.clone());
    match pred {
        Predicate::Eq(v) => {
            // Gremlin: P.eq(null) means IS NULL (unlike Cypher where null=null is null)
            if matches!(v, GremlinValue::Null) {
                return Expr::IsNull(Box::new(var));
            }
            // Gremlin: null.eq(non_null) is false, not null.
            // Use (var IS NOT NULL AND var == val) to prevent null from passing.
            Expr::BinaryOp {
                op: BinaryOp::And,
                left: Box::new(Expr::IsNotNull(Box::new(Expr::Variable(alias.clone())))),
                right: Box::new(Expr::BinaryOp {
                    op: BinaryOp::Eq,
                    left: Box::new(var),
                    right: Box::new(gremlin_value_to_expr(v)),
                }),
            }
        }
        Predicate::Neq(v) => {
            if matches!(v, GremlinValue::Null) {
                return Expr::IsNotNull(Box::new(var));
            }
            // Gremlin: null != non_null is true (pass through).
            // Use (var IS NULL OR var != val) to handle null propagation.
            Expr::BinaryOp {
                op: BinaryOp::Or,
                left: Box::new(Expr::IsNull(Box::new(Expr::Variable(alias.clone())))),
                right: Box::new(Expr::BinaryOp {
                    op: BinaryOp::Neq,
                    left: Box::new(var),
                    right: Box::new(gremlin_value_to_expr(v)),
                }),
            }
        }
        Predicate::Gt(v) => {
            if matches!(v, GremlinValue::Null) {
                return Expr::Bool(false);
            }
            Expr::BinaryOp {
                op: BinaryOp::Gt,
                left: Box::new(var),
                right: Box::new(gremlin_value_to_expr(v)),
            }
        }
        Predicate::Gte(v) => {
            // Gremlin: P.gte(null) on null value should be true
            if matches!(v, GremlinValue::Null) {
                return Expr::IsNull(Box::new(var));
            }
            Expr::BinaryOp {
                op: BinaryOp::Gte,
                left: Box::new(var),
                right: Box::new(gremlin_value_to_expr(v)),
            }
        }
        Predicate::Lt(v) => {
            // Gremlin: P.lt(null) always false (nothing is strictly less than null)
            if matches!(v, GremlinValue::Null) {
                return Expr::Bool(false);
            }
            Expr::BinaryOp {
                op: BinaryOp::Lt,
                left: Box::new(var),
                right: Box::new(gremlin_value_to_expr(v)),
            }
        }
        Predicate::Lte(v) => {
            if matches!(v, GremlinValue::Null) {
                return Expr::IsNull(Box::new(var));
            }
            Expr::BinaryOp {
                op: BinaryOp::Lte,
                left: Box::new(var),
                right: Box::new(gremlin_value_to_expr(v)),
            }
        }
        Predicate::Within(vals) => Expr::BinaryOp {
            op: BinaryOp::In,
            left: Box::new(var),
            right: Box::new(Expr::List(vals.iter().map(gremlin_value_to_expr).collect())),
        },
        Predicate::Without(vals) => Expr::UnaryOp {
            op: cypher_core::ast::UnaryOp::Not,
            expr: Box::new(Expr::BinaryOp {
                op: BinaryOp::In,
                left: Box::new(var),
                right: Box::new(Expr::List(vals.iter().map(gremlin_value_to_expr).collect())),
            }),
        },
        Predicate::Between(low, high) => Expr::BinaryOp {
            op: BinaryOp::And,
            left: Box::new(Expr::BinaryOp {
                op: BinaryOp::Gte,
                left: Box::new(Expr::Variable(alias.clone())),
                right: Box::new(gremlin_value_to_expr(low)),
            }),
            right: Box::new(Expr::BinaryOp {
                op: BinaryOp::Lt,
                left: Box::new(Expr::Variable(alias.clone())),
                right: Box::new(gremlin_value_to_expr(high)),
            }),
        },
        // inside(low, high) → low < x < high (exclusive on both ends)
        Predicate::Inside(low, high) => Expr::BinaryOp {
            op: BinaryOp::And,
            left: Box::new(Expr::BinaryOp {
                op: BinaryOp::Gt,
                left: Box::new(Expr::Variable(alias.clone())),
                right: Box::new(gremlin_value_to_expr(low)),
            }),
            right: Box::new(Expr::BinaryOp {
                op: BinaryOp::Lt,
                left: Box::new(Expr::Variable(alias.clone())),
                right: Box::new(gremlin_value_to_expr(high)),
            }),
        },
        // Text predicates reuse the same pattern as predicate_to_prop_expr
        Predicate::Containing(_)
        | Predicate::NotContaining(_)
        | Predicate::StartingWith(_)
        | Predicate::NotStartingWith(_)
        | Predicate::EndingWith(_)
        | Predicate::NotEndingWith(_)
        | Predicate::Regex(_)
        | Predicate::NotRegex(_)
        | Predicate::IsType(_)
        | Predicate::And(_)
        | Predicate::Or(_)
        | Predicate::Not(_) => predicate_to_prop_expr(pred, var),
    }
}

/// Apply a predicate to a property expression (for `has("key", P.gt(30))`).
pub(in crate::compiler) fn predicate_to_prop_expr(pred: &Predicate, prop_expr: Expr) -> Expr {
    match pred {
        Predicate::Eq(v) => Expr::BinaryOp {
            op: BinaryOp::Eq,
            left: Box::new(prop_expr),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Neq(v) => Expr::BinaryOp {
            op: BinaryOp::Neq,
            left: Box::new(prop_expr),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Gt(v) => Expr::BinaryOp {
            op: BinaryOp::Gt,
            left: Box::new(prop_expr),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Gte(v) => Expr::BinaryOp {
            op: BinaryOp::Gte,
            left: Box::new(prop_expr),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Lt(v) => Expr::BinaryOp {
            op: BinaryOp::Lt,
            left: Box::new(prop_expr),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Lte(v) => Expr::BinaryOp {
            op: BinaryOp::Lte,
            left: Box::new(prop_expr),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Within(vals) => Expr::BinaryOp {
            op: BinaryOp::In,
            left: Box::new(prop_expr),
            right: Box::new(Expr::List(vals.iter().map(gremlin_value_to_expr).collect())),
        },
        Predicate::Without(vals) => Expr::UnaryOp {
            op: cypher_core::ast::UnaryOp::Not,
            expr: Box::new(Expr::BinaryOp {
                op: BinaryOp::In,
                left: Box::new(prop_expr),
                right: Box::new(Expr::List(vals.iter().map(gremlin_value_to_expr).collect())),
            }),
        },
        Predicate::Between(low, high) => Expr::BinaryOp {
            op: BinaryOp::And,
            left: Box::new(Expr::BinaryOp {
                op: BinaryOp::Gte,
                left: Box::new(prop_expr.clone()),
                right: Box::new(gremlin_value_to_expr(low)),
            }),
            right: Box::new(Expr::BinaryOp {
                op: BinaryOp::Lt,
                left: Box::new(prop_expr),
                right: Box::new(gremlin_value_to_expr(high)),
            }),
        },
        // inside(low, high) → low < x < high (exclusive on both ends)
        Predicate::Inside(low, high) => Expr::BinaryOp {
            op: BinaryOp::And,
            left: Box::new(Expr::BinaryOp {
                op: BinaryOp::Gt,
                left: Box::new(prop_expr.clone()),
                right: Box::new(gremlin_value_to_expr(low)),
            }),
            right: Box::new(Expr::BinaryOp {
                op: BinaryOp::Lt,
                left: Box::new(prop_expr),
                right: Box::new(gremlin_value_to_expr(high)),
            }),
        },
        Predicate::Containing(v) => Expr::FunctionCall {
            name: SmolStr::new("gremlin_containing"),
            args: vec![prop_expr, gremlin_value_to_expr(v)],
            distinct: false,
        },
        Predicate::NotContaining(v) => Expr::UnaryOp {
            op: cypher_core::ast::UnaryOp::Not,
            expr: Box::new(Expr::FunctionCall {
                name: SmolStr::new("gremlin_containing"),
                args: vec![prop_expr, gremlin_value_to_expr(v)],
                distinct: false,
            }),
        },
        Predicate::StartingWith(v) => Expr::FunctionCall {
            name: SmolStr::new("gremlin_startingWith"),
            args: vec![prop_expr, gremlin_value_to_expr(v)],
            distinct: false,
        },
        Predicate::NotStartingWith(v) => Expr::UnaryOp {
            op: cypher_core::ast::UnaryOp::Not,
            expr: Box::new(Expr::FunctionCall {
                name: SmolStr::new("gremlin_startingWith"),
                args: vec![prop_expr, gremlin_value_to_expr(v)],
                distinct: false,
            }),
        },
        Predicate::EndingWith(v) => Expr::FunctionCall {
            name: SmolStr::new("gremlin_endingWith"),
            args: vec![prop_expr, gremlin_value_to_expr(v)],
            distinct: false,
        },
        Predicate::NotEndingWith(v) => Expr::UnaryOp {
            op: cypher_core::ast::UnaryOp::Not,
            expr: Box::new(Expr::FunctionCall {
                name: SmolStr::new("gremlin_endingWith"),
                args: vec![prop_expr, gremlin_value_to_expr(v)],
                distinct: false,
            }),
        },
        Predicate::Regex(v) => Expr::FunctionCall {
            name: SmolStr::new("gremlin_regex"),
            args: vec![prop_expr, gremlin_value_to_expr(v)],
            distinct: false,
        },
        Predicate::NotRegex(v) => Expr::UnaryOp {
            op: cypher_core::ast::UnaryOp::Not,
            expr: Box::new(Expr::FunctionCall {
                name: SmolStr::new("gremlin_regex"),
                args: vec![prop_expr, gremlin_value_to_expr(v)],
                distinct: false,
            }),
        },
        Predicate::IsType(v) => Expr::FunctionCall {
            name: SmolStr::new("gremlin_istype"),
            args: vec![prop_expr, gremlin_value_to_expr(v)],
            distinct: false,
        },
        Predicate::And(preds) => {
            let mut iter = preds.iter();
            let first = predicate_to_prop_expr(iter.next().unwrap(), prop_expr.clone());
            iter.fold(first, |acc, p| Expr::BinaryOp {
                op: BinaryOp::And,
                left: Box::new(acc),
                right: Box::new(predicate_to_prop_expr(p, prop_expr.clone())),
            })
        }
        Predicate::Or(preds) => {
            let mut iter = preds.iter();
            let first = predicate_to_prop_expr(iter.next().unwrap(), prop_expr.clone());
            iter.fold(first, |acc, p| Expr::BinaryOp {
                op: BinaryOp::Or,
                left: Box::new(acc),
                right: Box::new(predicate_to_prop_expr(p, prop_expr.clone())),
            })
        }
        Predicate::Not(inner) => Expr::UnaryOp {
            op: cypher_core::ast::UnaryOp::Not,
            expr: Box::new(predicate_to_prop_expr(inner, prop_expr)),
        },
    }
}
