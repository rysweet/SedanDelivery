//! Label-aware predicate analysis and operator extraction.
//!
//! Extracted from `helpers.rs` as part of the helpers/ submodule split.
//! All items are `pub(in crate::compiler)` and behave identically to their pre-split
//! definitions.

use std::collections::{HashMap, HashSet};

use cypher_core::ast::{BinaryOp, Expr, Identifier};

use crate::bytecode::Predicate;
use crate::types::GremlinValue;

use super::*;

pub(in crate::compiler) fn predicate_string_ref(pred: &Predicate) -> Option<&str> {
    match pred {
        Predicate::Eq(GremlinValue::String(s))
        | Predicate::Neq(GremlinValue::String(s))
        | Predicate::Gt(GremlinValue::String(s))
        | Predicate::Gte(GremlinValue::String(s))
        | Predicate::Lt(GremlinValue::String(s))
        | Predicate::Lte(GremlinValue::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Check if any leaf predicate in a compound predicate references a known label.
pub(in crate::compiler) fn predicate_has_label_refs(
    pred: &Predicate,
    alias_map: &HashMap<String, Identifier>,
    aggregate_keys: &HashSet<String>,
) -> bool {
    match pred {
        Predicate::Eq(GremlinValue::String(s))
        | Predicate::Neq(GremlinValue::String(s))
        | Predicate::Gt(GremlinValue::String(s))
        | Predicate::Gte(GremlinValue::String(s))
        | Predicate::Lt(GremlinValue::String(s))
        | Predicate::Lte(GremlinValue::String(s)) => alias_map.contains_key(s.as_str()),
        // within("key") / without("key") with a single string arg that names
        // an aggregate side-effect or alias counts as a label reference.
        Predicate::Within(vals) | Predicate::Without(vals) if vals.len() == 1 => {
            if let GremlinValue::String(s) = &vals[0] {
                alias_map.contains_key(s.as_str()) || aggregate_keys.contains(s.as_str())
            } else {
                false
            }
        }
        Predicate::Or(preds) | Predicate::And(preds) => preds
            .iter()
            .any(|p| predicate_has_label_refs(p, alias_map, aggregate_keys)),
        Predicate::Not(inner) => predicate_has_label_refs(inner, alias_map, aggregate_keys),
        _ => false,
    }
}

/// Build a comparison expression for a compound predicate where each leaf
/// may reference a different label or aggregate side-effect key.
/// Resolves each label/key to its variable.
pub(in crate::compiler) fn predicate_to_label_aware_expr(
    pred: &Predicate,
    left: &Expr,
    alias_map: &HashMap<String, Identifier>,
    aggregate_keys: &HashSet<String>,
) -> Expr {
    match pred {
        Predicate::Eq(GremlinValue::String(s))
        | Predicate::Neq(GremlinValue::String(s))
        | Predicate::Gt(GremlinValue::String(s))
        | Predicate::Gte(GremlinValue::String(s))
        | Predicate::Lt(GremlinValue::String(s))
        | Predicate::Lte(GremlinValue::String(s)) => {
            let right = if let Some(id) = alias_map.get(s.as_str()) {
                Expr::Variable(id.clone())
            } else {
                gremlin_value_to_expr(&GremlinValue::String(s.clone()))
            };
            let op = predicate_to_binary_op(pred);
            Expr::BinaryOp {
                op,
                left: Box::new(left.clone()),
                right: Box::new(right),
            }
        }
        // within("key") where "key" is an aggregate side-effect: left IN __gremlin_agg_key
        Predicate::Within(vals) if vals.len() == 1 => {
            if let GremlinValue::String(s) = &vals[0] {
                if let Some(id) = alias_map.get(s.as_str()) {
                    return Expr::BinaryOp {
                        op: BinaryOp::In,
                        left: Box::new(left.clone()),
                        right: Box::new(Expr::Variable(id.clone())),
                    };
                }
                if aggregate_keys.contains(s.as_str()) {
                    return Expr::BinaryOp {
                        op: BinaryOp::In,
                        left: Box::new(left.clone()),
                        right: Box::new(Expr::Variable(Identifier::new(format!(
                            "__gremlin_agg_{s}"
                        )))),
                    };
                }
            }
            predicate_to_prop_expr(pred, left.clone())
        }
        // without("key") where "key" is an aggregate side-effect: NOT (left IN __gremlin_agg_key)
        Predicate::Without(vals) if vals.len() == 1 => {
            if let GremlinValue::String(s) = &vals[0] {
                if let Some(id) = alias_map.get(s.as_str()) {
                    return Expr::UnaryOp {
                        op: cypher_core::ast::UnaryOp::Not,
                        expr: Box::new(Expr::BinaryOp {
                            op: BinaryOp::In,
                            left: Box::new(left.clone()),
                            right: Box::new(Expr::Variable(id.clone())),
                        }),
                    };
                }
                if aggregate_keys.contains(s.as_str()) {
                    return Expr::UnaryOp {
                        op: cypher_core::ast::UnaryOp::Not,
                        expr: Box::new(Expr::BinaryOp {
                            op: BinaryOp::In,
                            left: Box::new(left.clone()),
                            right: Box::new(Expr::Variable(Identifier::new(format!(
                                "__gremlin_agg_{s}"
                            )))),
                        }),
                    };
                }
            }
            predicate_to_prop_expr(pred, left.clone())
        }
        Predicate::Or(preds) => {
            let mut iter = preds.iter();
            let first = predicate_to_label_aware_expr(
                iter.next().unwrap(),
                left,
                alias_map,
                aggregate_keys,
            );
            iter.fold(first, |acc, p| Expr::BinaryOp {
                op: BinaryOp::Or,
                left: Box::new(acc),
                right: Box::new(predicate_to_label_aware_expr(
                    p,
                    left,
                    alias_map,
                    aggregate_keys,
                )),
            })
        }
        Predicate::And(preds) => {
            let mut iter = preds.iter();
            let first = predicate_to_label_aware_expr(
                iter.next().unwrap(),
                left,
                alias_map,
                aggregate_keys,
            );
            iter.fold(first, |acc, p| Expr::BinaryOp {
                op: BinaryOp::And,
                left: Box::new(acc),
                right: Box::new(predicate_to_label_aware_expr(
                    p,
                    left,
                    alias_map,
                    aggregate_keys,
                )),
            })
        }
        Predicate::Not(inner) => Expr::UnaryOp {
            op: cypher_core::ast::UnaryOp::Not,
            expr: Box::new(predicate_to_label_aware_expr(
                inner,
                left,
                alias_map,
                aggregate_keys,
            )),
        },
        _ => predicate_to_prop_expr(pred, left.clone()),
    }
}

/// in the alias map or aggregate keys. Returns the variable expression for that label if so.
pub(in crate::compiler) fn predicate_label_ref(
    pred: &Predicate,
    alias_map: &HashMap<String, Identifier>,
    aggregate_keys: &HashSet<String>,
) -> Option<Expr> {
    let label = match pred {
        Predicate::Eq(GremlinValue::String(s))
        | Predicate::Neq(GremlinValue::String(s))
        | Predicate::Gt(GremlinValue::String(s))
        | Predicate::Gte(GremlinValue::String(s))
        | Predicate::Lt(GremlinValue::String(s))
        | Predicate::Lte(GremlinValue::String(s)) => s,
        // within("a") / without("a") with a single string argument:
        // if it resolves to a known alias/side-effect, treat it as a
        // variable reference (membership test against the stored list).
        Predicate::Within(vals) if vals.len() == 1 => {
            if let GremlinValue::String(s) = &vals[0] {
                s
            } else {
                return None;
            }
        }
        Predicate::Without(vals) if vals.len() == 1 => {
            if let GremlinValue::String(s) = &vals[0] {
                s
            } else {
                return None;
            }
        }
        _ => return None,
    };
    // Check alias_map first (as-labels, side-effects)
    if let Some(id) = alias_map.get(label) {
        return Some(Expr::Variable(id.clone()));
    }
    // Check aggregate keys — resolve to __gremlin_agg_<key>
    if aggregate_keys.contains(label) {
        return Some(Expr::Variable(Identifier::new(format!(
            "__gremlin_agg_{label}"
        ))));
    }
    None
}

/// Extract the BinaryOp from a predicate (ignoring its value).
pub(in crate::compiler) fn predicate_to_binary_op(pred: &Predicate) -> BinaryOp {
    match pred {
        Predicate::Eq(_) => BinaryOp::Eq,
        Predicate::Neq(_) => BinaryOp::Neq,
        Predicate::Gt(_) => BinaryOp::Gt,
        Predicate::Gte(_) => BinaryOp::Gte,
        Predicate::Lt(_) => BinaryOp::Lt,
        Predicate::Lte(_) => BinaryOp::Lte,
        Predicate::Within(_) => BinaryOp::In,
        Predicate::Without(_) => BinaryOp::In,
        Predicate::Between(_, _) => BinaryOp::And,
        Predicate::Inside(_, _) => BinaryOp::And,
        // Text predicates don't map to a single BinaryOp; default to Eq
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
        | Predicate::Not(_) => BinaryOp::Eq,
    }
}
