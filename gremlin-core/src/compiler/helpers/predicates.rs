//! Conversion of `GremlinStep`/step sequences into filter `Expr` predicates.
//!
//! Extracted from `helpers.rs` as part of the helpers/ submodule split.
//! All items are `pub(in crate::compiler)` and behave identically to their pre-split
//! definitions.

use std::collections::{HashMap, HashSet};

use cypher_core::ast::{BinaryOp, Expr, Identifier, Label, UnaryOp};
use smol_str::SmolStr;

use crate::bytecode::{GremlinStep, Predicate};
use crate::types::GremlinValue;

use super::*;

/// Convert a single GremlinStep into a filter predicate Expr.
pub(in crate::compiler) fn step_to_predicate(step: &GremlinStep, alias: &Identifier) -> Expr {
    match step {
        GremlinStep::Has(key, Some(val)) => Expr::BinaryOp {
            op: BinaryOp::Eq,
            left: Box::new(Expr::Property(
                Box::new(Expr::Variable(alias.clone())),
                SmolStr::new(key),
            )),
            right: Box::new(gremlin_value_to_expr(val)),
        },
        GremlinStep::Has(key, None) => Expr::IsNotNull(Box::new(Expr::Property(
            Box::new(Expr::Variable(alias.clone())),
            SmolStr::new(key),
        ))),
        GremlinStep::HasPredicate(key, pred) => {
            let prop_expr =
                Expr::Property(Box::new(Expr::Variable(alias.clone())), SmolStr::new(key));
            // has(key, pred) means: property exists AND satisfies predicate.
            // Wrap in AND(IsNotNull(prop), pred(prop)) to properly handle
            // missing properties (returns false instead of null).
            let pred_expr = predicate_to_prop_expr(pred, prop_expr.clone());
            Expr::BinaryOp {
                op: BinaryOp::And,
                left: Box::new(Expr::IsNotNull(Box::new(prop_expr))),
                right: Box::new(pred_expr),
            }
        }
        GremlinStep::HasLabel(labels) if !labels.is_empty() => labels
            .iter()
            .map(|l| {
                Expr::HasLabel(
                    Box::new(Expr::Variable(alias.clone())),
                    Label::new(l.as_str()),
                )
            })
            .reduce(|a, b| Expr::BinaryOp {
                op: BinaryOp::Or,
                left: Box::new(a),
                right: Box::new(b),
            })
            .unwrap(),
        GremlinStep::Is(pred) => predicate_to_expr(pred, alias),
        GremlinStep::HasId(ids) if !ids.is_empty() => {
            let id_fn = Expr::FunctionCall {
                name: SmolStr::new("id"),
                args: vec![Expr::Variable(alias.clone())],
                distinct: false,
            };
            // Flatten list parameters: hasId(id1, [id2, id3]) → hasId(id1, id2, id3)
            let mut flat_ids = Vec::new();
            for id in ids {
                match id {
                    GremlinValue::List(inner) => flat_ids.extend(inner.iter().cloned()),
                    other => flat_ids.push(other.clone()),
                }
            }
            if flat_ids.len() == 1 {
                Expr::BinaryOp {
                    op: BinaryOp::Eq,
                    left: Box::new(id_fn),
                    right: Box::new(gremlin_value_to_expr(&flat_ids[0])),
                }
            } else {
                let id_exprs: Vec<Expr> = flat_ids.iter().map(gremlin_value_to_expr).collect();
                Expr::BinaryOp {
                    op: BinaryOp::In,
                    left: Box::new(id_fn),
                    right: Box::new(Expr::List(id_exprs)),
                }
            }
        }
        GremlinStep::HasIdPredicate(pred) => {
            let id_fn = Expr::FunctionCall {
                name: SmolStr::new("id"),
                args: vec![Expr::Variable(alias.clone())],
                distinct: false,
            };
            predicate_to_prop_expr(pred, id_fn)
        }
        // Non-filter steps produce vacuous truth. Complex sub-traversals
        // are routed through Apply+Count existence checks by callers
        // (Where/Not) before reaching this function.
        _ => Expr::Bool(true),
    }
}

/// Convert a list of GremlinSteps into a filter predicate.
/// Handles multi-step traversals like `values("age").is(P.gt(30))`.
pub(in crate::compiler) fn steps_to_predicate(
    steps: &[GremlinStep],
    alias: &Identifier,
    alias_map: &HashMap<String, Identifier>,
    aggregate_keys: &HashSet<String>,
) -> Expr {
    if steps.is_empty() {
        return Expr::Bool(true);
    }

    // Single step: delegate to step_to_predicate
    if steps.len() == 1 {
        // Handle Or/And/Not at the single-step level too
        match &steps[0] {
            GremlinStep::Or(branches) => {
                return branches
                    .iter()
                    .map(|b| steps_to_predicate(b, alias, alias_map, aggregate_keys))
                    .reduce(|a, b| Expr::BinaryOp {
                        op: BinaryOp::Or,
                        left: Box::new(a),
                        right: Box::new(b),
                    })
                    .unwrap_or(Expr::Bool(false));
            }
            GremlinStep::And(branches) => {
                return branches
                    .iter()
                    .map(|b| steps_to_predicate(b, alias, alias_map, aggregate_keys))
                    .reduce(|a, b| Expr::BinaryOp {
                        op: BinaryOp::And,
                        left: Box::new(a),
                        right: Box::new(b),
                    })
                    .unwrap_or(Expr::Bool(true));
            }
            GremlinStep::Not(inner) => {
                return Expr::UnaryOp {
                    op: UnaryOp::Not,
                    expr: Box::new(steps_to_predicate(inner, alias, alias_map, aggregate_keys)),
                };
            }
            _ => {}
        }
        // Single Is(pred) where the pred argument is a string label reference:
        // where(P.eq("b")) → compare current value against labeled value "b"
        if let GremlinStep::Is(pred) = &steps[0] {
            // Handle Not(inner) by unwrapping: resolve the inner predicate's
            // label reference and then negate the result.
            let (negated, inner_pred) = match pred {
                Predicate::Not(inner) => (true, inner.as_ref()),
                _ => (false, pred),
            };
            if let Some(label_expr) = predicate_label_ref(inner_pred, alias_map, aggregate_keys) {
                let result = match inner_pred {
                    // within("key") / without("key") — membership test against the
                    // stored list.  within → alias IN key; without → NOT(alias IN key).
                    Predicate::Within(_) | Predicate::Without(_) => {
                        let is_without = matches!(inner_pred, Predicate::Without(_));
                        let cmp = Expr::BinaryOp {
                            op: BinaryOp::In,
                            left: Box::new(Expr::Variable(alias.clone())),
                            right: Box::new(label_expr),
                        };
                        // Negate for Without or when the outer predicate was negated
                        // (XOR so double-negation resolves correctly).
                        if is_without ^ negated {
                            Expr::UnaryOp {
                                op: UnaryOp::Not,
                                expr: Box::new(cmp),
                            }
                        } else {
                            cmp
                        }
                    }
                    // eq("a") / neq("a") / gt("a") etc. — direct comparison against
                    // the labeled value.
                    _ => {
                        let op = predicate_to_binary_op(inner_pred);
                        let cmp = Expr::BinaryOp {
                            op,
                            left: Box::new(Expr::Variable(alias.clone())),
                            right: Box::new(label_expr),
                        };
                        if negated {
                            Expr::UnaryOp {
                                op: UnaryOp::Not,
                                expr: Box::new(cmp),
                            }
                        } else {
                            cmp
                        }
                    }
                };
                return result;
            }
            // Also try predicate_has_label_refs + predicate_to_label_aware_expr
            // for compound predicates like P.not(P.neq("a"))
            if predicate_has_label_refs(pred, alias_map, aggregate_keys) {
                return predicate_to_label_aware_expr(
                    pred,
                    &Expr::Variable(alias.clone()),
                    alias_map,
                    aggregate_keys,
                );
            }
        }
        return step_to_predicate(&steps[0], alias);
    }

    // Two-step pattern: Values(["key"]) + Is(pred) → property predicate
    if steps.len() == 2 {
        // loops("label").is(pred) and loops().is(pred) must target the
        // executor-injected loop counter columns, not the current traverser.
        if let (GremlinStep::Loops(label), GremlinStep::Is(pred)) = (&steps[0], &steps[1]) {
            let loop_var = match label {
                Some(l) => Identifier::new(format!("__gremlin_loop_count_{l}")),
                None => Identifier::new("__gremlin_loop_count"),
            };
            return predicate_to_expr(pred, &loop_var);
        }
        if let (GremlinStep::Values(keys), GremlinStep::Is(pred)) = (&steps[0], &steps[1]) {
            if keys.len() == 1 {
                let prop_expr = Expr::Property(
                    Box::new(Expr::Variable(alias.clone())),
                    SmolStr::new(&keys[0]),
                );
                return predicate_to_prop_expr(pred, prop_expr);
            }
        }
        // Label + Is(Eq("person")) → hasLabel("person")
        if let (GremlinStep::Label, GremlinStep::Is(pred)) = (&steps[0], &steps[1]) {
            match pred {
                Predicate::Eq(GremlinValue::String(label)) => {
                    return Expr::HasLabel(
                        Box::new(Expr::Variable(alias.clone())),
                        Label::new(label.as_str()),
                    );
                }
                _ => {
                    // For other predicates on label, use function call
                    let label_expr = Expr::FunctionCall {
                        name: SmolStr::new("labels"),
                        args: vec![Expr::Variable(alias.clone())],
                        distinct: false,
                    };
                    return predicate_to_prop_expr(pred, label_expr);
                }
            }
        }
        // Id + Is(pred) → id predicate
        if let (GremlinStep::Id, GremlinStep::Is(pred)) = (&steps[0], &steps[1]) {
            let id_expr = Expr::FunctionCall {
                name: SmolStr::new("id"),
                args: vec![Expr::Variable(alias.clone())],
                distinct: false,
            };
            return predicate_to_prop_expr(pred, id_expr);
        }
        // Two-step pattern: Select(["label"]) + Is(pred) → labeled value comparison
        // This handles where("a", P.eq("b"))
        if let (GremlinStep::Select(_, keys), GremlinStep::Is(pred)) = (&steps[0], &steps[1]) {
            if keys.len() == 1 {
                let src_expr = if keys[0] == "keys" || keys[0] == "Column.keys" {
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_unfold_map_entry_key"),
                        args: vec![Expr::Variable(alias.clone())],
                        distinct: false,
                    }
                } else if keys[0] == "values" || keys[0] == "Column.values" {
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_unfold_map_entry_value"),
                        args: vec![Expr::Variable(alias.clone())],
                        distinct: false,
                    }
                } else {
                    Expr::Variable(
                        alias_map
                            .get(&keys[0])
                            .cloned()
                            .unwrap_or_else(|| Identifier::new(keys[0].clone())),
                    )
                };
                if let Some(label_expr) = predicate_label_ref(pred, alias_map, aggregate_keys) {
                    let op = predicate_to_binary_op(pred);
                    return Expr::BinaryOp {
                        op,
                        left: Box::new(src_expr.clone()),
                        right: Box::new(label_expr),
                    };
                }
                if let Some(column_name) = predicate_string_ref(pred) {
                    let op = predicate_to_binary_op(pred);
                    return Expr::BinaryOp {
                        op,
                        left: Box::new(src_expr.clone()),
                        right: Box::new(Expr::Variable(Identifier::new(column_name))),
                    };
                }
                // For compound predicates (Or/And/Not) that reference labels,
                // build the expression resolving each label to its variable.
                if predicate_has_label_refs(pred, alias_map, aggregate_keys) {
                    return predicate_to_label_aware_expr(
                        pred,
                        &src_expr,
                        alias_map,
                        aggregate_keys,
                    );
                }
                if let Expr::Variable(src_var) = src_expr {
                    return predicate_to_expr(pred, &src_var);
                }
                return predicate_to_prop_expr(pred, src_expr);
            }
        }
    }

    // Multi-step patterns: try to build AND of individual predicates
    let mut preds = Vec::new();
    for step in steps {
        // Handle Or/And/Not by recursing into steps_to_predicate
        let pred = match step {
            GremlinStep::Or(branches) => branches
                .iter()
                .map(|b| steps_to_predicate(b, alias, alias_map, aggregate_keys))
                .reduce(|a, b| Expr::BinaryOp {
                    op: BinaryOp::Or,
                    left: Box::new(a),
                    right: Box::new(b),
                })
                .unwrap_or(Expr::Bool(false)),
            GremlinStep::And(branches) => branches
                .iter()
                .map(|b| steps_to_predicate(b, alias, alias_map, aggregate_keys))
                .reduce(|a, b| Expr::BinaryOp {
                    op: BinaryOp::And,
                    left: Box::new(a),
                    right: Box::new(b),
                })
                .unwrap_or(Expr::Bool(true)),
            GremlinStep::Not(inner) => {
                let inner_pred = steps_to_predicate(inner, alias, alias_map, aggregate_keys);
                Expr::UnaryOp {
                    op: UnaryOp::Not,
                    expr: Box::new(inner_pred),
                }
            }
            _ => step_to_predicate(step, alias),
        };
        if pred != Expr::Bool(true) {
            preds.push(pred);
        }
    }

    if preds.is_empty() {
        return Expr::Bool(true);
    }

    preds
        .into_iter()
        .reduce(|a, b| Expr::BinaryOp {
            op: BinaryOp::And,
            left: Box::new(a),
            right: Box::new(b),
        })
        .unwrap_or(Expr::Bool(true))
}
