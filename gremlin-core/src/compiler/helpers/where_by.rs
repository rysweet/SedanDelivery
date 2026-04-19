//! `where(...).by(...)` modulation handling and the `WhereByTarget` enum.
//!
//! Extracted from `helpers.rs` as part of the helpers/ submodule split.
//! All items are `pub(in crate::compiler)` and behave identically to their pre-split
//! definitions.

use std::collections::HashMap;

use cypher_core::ast::{BinaryOp, Expr, Identifier};
use smol_str::SmolStr;

use crate::bytecode::{GremlinStep, Predicate};
use crate::types::GremlinValue;

#[derive(Clone)]
pub(in crate::compiler) enum WhereByTarget {
    Identifier(Identifier),
    MapEntryKey(Identifier),
    MapEntryValue(Identifier),
}

impl WhereByTarget {
    fn matches_variable(&self, variable: &Identifier) -> bool {
        match self {
            WhereByTarget::Identifier(id)
            | WhereByTarget::MapEntryKey(id)
            | WhereByTarget::MapEntryValue(id) => id == variable,
        }
    }

    fn to_expr(&self) -> Expr {
        match self {
            WhereByTarget::Identifier(id) => Expr::Variable(id.clone()),
            WhereByTarget::MapEntryKey(id) => Expr::FunctionCall {
                name: SmolStr::new("gremlin_unfold_map_entry_key"),
                args: vec![Expr::Variable(id.clone())],
                distinct: false,
            },
            WhereByTarget::MapEntryValue(id) => Expr::FunctionCall {
                name: SmolStr::new("gremlin_unfold_map_entry_value"),
                args: vec![Expr::Variable(id.clone())],
                distinct: false,
            },
        }
    }
}

pub(in crate::compiler) fn extract_where_by_targets(
    steps: &[GremlinStep],
    alias_map: &HashMap<String, Identifier>,
    current_alias: &Identifier,
) -> Option<Vec<WhereByTarget>> {
    // Pattern 1: Select(["a"]) + Is(pred("b")) → where("a", P.gt("b"))
    if steps.len() == 2 {
        if let (GremlinStep::Select(_, keys), GremlinStep::Is(pred)) = (&steps[0], &steps[1]) {
            if keys.len() == 1 {
                let left = if keys[0] == "keys" || keys[0] == "Column.keys" {
                    WhereByTarget::MapEntryKey(current_alias.clone())
                } else if keys[0] == "values" || keys[0] == "Column.values" {
                    WhereByTarget::MapEntryValue(current_alias.clone())
                } else {
                    WhereByTarget::Identifier(
                        alias_map
                            .get(&keys[0])
                            .cloned()
                            .unwrap_or_else(|| Identifier::new(keys[0].clone())),
                    )
                };
                let mut targets = vec![left];
                collect_predicate_labels_in_order(pred, alias_map, &mut targets);
                if targets.len() > 1 {
                    return Some(targets);
                }
            }
        }
    }
    // Pattern 2: Is(pred("a")) → where(P.gt("a")) compares current vs label
    if steps.len() == 1 {
        if let GremlinStep::Is(pred) = &steps[0] {
            let mut targets = vec![WhereByTarget::Identifier(current_alias.clone())];
            collect_predicate_labels_in_order(pred, alias_map, &mut targets);
            if targets.len() > 1 {
                return Some(targets);
            }
        }
    }
    None
}

pub(in crate::compiler) fn collect_predicate_labels_in_order(
    pred: &Predicate,
    alias_map: &HashMap<String, Identifier>,
    targets: &mut Vec<WhereByTarget>,
) {
    match pred {
        Predicate::Eq(GremlinValue::String(s))
        | Predicate::Neq(GremlinValue::String(s))
        | Predicate::Gt(GremlinValue::String(s))
        | Predicate::Gte(GremlinValue::String(s))
        | Predicate::Lt(GremlinValue::String(s))
        | Predicate::Lte(GremlinValue::String(s)) => {
            if let Some(id) = alias_map.get(s.as_str()) {
                if !targets
                    .iter()
                    .any(|existing| matches!(existing, WhereByTarget::Identifier(existing_id) if existing_id == id))
                {
                    targets.push(WhereByTarget::Identifier(id.clone()));
                }
            }
        }
        Predicate::Within(vals) | Predicate::Without(vals) if vals.len() == 1 => {
            if let GremlinValue::String(s) = &vals[0] {
                if let Some(id) = alias_map.get(s.as_str()) {
                    if !targets
                        .iter()
                        .any(|existing| matches!(existing, WhereByTarget::Identifier(existing_id) if existing_id == id))
                    {
                        targets.push(WhereByTarget::Identifier(id.clone()));
                    }
                }
            }
        }
        Predicate::Or(preds) | Predicate::And(preds) => {
            for inner in preds {
                collect_predicate_labels_in_order(inner, alias_map, targets);
            }
        }
        Predicate::Not(inner) => collect_predicate_labels_in_order(inner, alias_map, targets),
        _ => {}
    }
}

pub(in crate::compiler) fn finalize_where_by_modulation(
    predicate: &mut Expr,
    targets: &[WhereByTarget],
    modulators: &[String],
    productive_by: bool,
) {
    if targets.is_empty() || modulators.is_empty() {
        return;
    }

    for (index, target_id) in targets.iter().enumerate() {
        let key = &modulators[index % modulators.len()];
        rewrite_where_by_target(predicate, target_id, key);
    }

    if productive_by {
        wrap_where_by_null_eq(predicate);
    }
}

pub(in crate::compiler) fn rewrite_where_by_target(
    predicate: &mut Expr,
    target: &WhereByTarget,
    key: &str,
) {
    match predicate {
        Expr::BinaryOp { left, right, .. } => {
            rewrite_where_by_expr(left, target, key);
            rewrite_where_by_expr(right, target, key);
        }
        Expr::UnaryOp { expr, .. } => rewrite_where_by_expr(expr, target, key),
        Expr::IsNull(expr) => rewrite_where_by_expr(expr, target, key),
        _ => {}
    }
}

pub(in crate::compiler) fn rewrite_where_by_expr(
    expr: &mut Expr,
    target: &WhereByTarget,
    key: &str,
) {
    match expr {
        Expr::Variable(v) if target.matches_variable(v) => {
            let base = target.to_expr();
            *expr = if key.is_empty() {
                base
            } else {
                Expr::Property(Box::new(base), SmolStr::new(key))
            };
        }
        Expr::Property(inner, _) => {
            if matches!(inner.as_ref(), Expr::Variable(v) if target.matches_variable(v)) {
                let base = target.to_expr();
                *expr = if key.is_empty() {
                    base
                } else {
                    Expr::Property(Box::new(base), SmolStr::new(key))
                };
            } else {
                rewrite_where_by_expr(inner, target, key);
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            rewrite_where_by_expr(left, target, key);
            rewrite_where_by_expr(right, target, key);
        }
        Expr::UnaryOp { expr: inner, .. } | Expr::IsNull(inner) => {
            rewrite_where_by_expr(inner, target, key);
        }
        _ => {}
    }
}

/// ProductiveByStrategy: for each Eq comparison leaf in a where-by predicate,
/// add `OR (left IS NULL AND right IS NULL)` so that null == null passes.
pub(in crate::compiler) fn wrap_where_by_null_eq(predicate: &mut Expr) {
    if let Expr::BinaryOp { op, left, right } = predicate {
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            wrap_where_by_null_eq(left);
            wrap_where_by_null_eq(right);
        } else if matches!(op, BinaryOp::Eq) {
            // Transform: (L == R) → (L == R) OR (L IS NULL AND R IS NULL)
            let left_clone = left.clone();
            let right_clone = right.clone();
            let null_check = Expr::BinaryOp {
                op: BinaryOp::And,
                left: Box::new(Expr::IsNull(left_clone)),
                right: Box::new(Expr::IsNull(right_clone)),
            };
            let original = std::mem::replace(predicate, Expr::Bool(true));
            *predicate = Expr::BinaryOp {
                op: BinaryOp::Or,
                left: Box::new(original),
                right: Box::new(null_check),
            };
        }
    }
}
