//! `filter` category step compilation arms extracted from `dispatch.rs`.
//!
//! Pure mechanical extraction — no behavior changes. Each arm body is
//! verbatim from the corresponding case in the original monolithic match.

#![allow(unused_imports)]

use std::collections::HashMap;

use cypher_core::ast::{BinaryOp, Direction, Expr, Identifier, Label, RelType};
use cypher_core::planner::{
    AggFunction, CreateEdgeSpec, CreatePattern, GremlinPop, Operator, SetItem,
};
use smol_str::SmolStr;

use super::super::compile_from_context::CompileFromContext;
use super::super::helpers::*;
use super::super::{CompileError, GremlinCompiler, SelectByModulator};
use super::StepFlow;
use crate::bytecode::{GremlinStep, Predicate};
use crate::types::GremlinValue;

impl GremlinCompiler {
    pub(super) fn compile_filter_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::HasLabel(labels) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                if labels.is_empty() {
                    // hasLabel() with no labels — no filtering needed
                    ctx.current = Some(child);
                } else if labels.len() == 1 {
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::HasLabel(
                            Box::new(Expr::Variable(ctx.current_alias.clone())),
                            Label::new(&labels[0]),
                        ),
                        child: Box::new(child),
                    });
                } else {
                    // OR of label checks
                    let mut checks: Vec<Expr> = labels
                        .iter()
                        .map(|l| {
                            Expr::HasLabel(
                                Box::new(Expr::Variable(ctx.current_alias.clone())),
                                Label::new(l.as_str()),
                            )
                        })
                        .collect();
                    let predicate = checks
                        .drain(..)
                        .reduce(|a, b| Expr::BinaryOp {
                            op: BinaryOp::Or,
                            left: Box::new(a),
                            right: Box::new(b),
                        })
                        .unwrap();
                    ctx.current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(child),
                    });
                }
            }

            GremlinStep::Has(key, value) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let prop_expr = gremlin_has_key_expr(key, &ctx.current_alias);
                let predicate = match value {
                    Some(val) => Expr::BinaryOp {
                        op: BinaryOp::Eq,
                        left: Box::new(prop_expr),
                        right: Box::new(if is_gremlin_id_key(key) {
                            gremlin_value_to_id_expr(val)
                        } else {
                            gremlin_value_to_expr(val)
                        }),
                    },
                    None => Expr::IsNotNull(Box::new(prop_expr)),
                };
                ctx.current = Some(Operator::Filter {
                    predicate,
                    child: Box::new(child),
                });
            }

            GremlinStep::HasPredicate(key, pred) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let prop_expr = gremlin_has_key_expr(key, &ctx.current_alias);
                let predicate = predicate_to_prop_expr(pred, prop_expr);
                ctx.current = Some(Operator::Filter {
                    predicate,
                    child: Box::new(child),
                });
            }

            GremlinStep::HasId(ids) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let id_fn = Expr::FunctionCall {
                    name: SmolStr::new("id"),
                    args: vec![Expr::Variable(ctx.current_alias.clone())],
                    distinct: false,
                };
                // Flatten list parameters: hasId(id1, [id2, id3]) → hasId(id1, id2, id3).
                // Also filter out null IDs: hasId(id, null) — TinkerPop semantics treat
                // null as "no match" so null values are silently dropped from the id set.
                let mut flat_ids = Vec::new();
                for id in ids {
                    match id {
                        GremlinValue::List(inner) => {
                            flat_ids.extend(
                                inner
                                    .iter()
                                    .filter(|v| !matches!(v, GremlinValue::Null))
                                    .cloned(),
                            );
                        }
                        GremlinValue::Null => {} // skip null ids
                        other => flat_ids.push(other.clone()),
                    }
                }
                let predicate = if flat_ids.len() == 1 {
                    Expr::BinaryOp {
                        op: BinaryOp::Eq,
                        left: Box::new(id_fn),
                        // Use id-aware conversion: string "1" → Integer(1)
                        right: Box::new(gremlin_value_to_id_expr(&flat_ids[0])),
                    }
                } else {
                    let id_exprs: Vec<Expr> =
                        flat_ids.iter().map(gremlin_value_to_id_expr).collect();
                    Expr::BinaryOp {
                        op: BinaryOp::In,
                        left: Box::new(id_fn),
                        right: Box::new(Expr::List(id_exprs)),
                    }
                };
                ctx.current = Some(Operator::Filter {
                    predicate,
                    child: Box::new(child),
                });
            }

            GremlinStep::HasIdPredicate(pred) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let id_fn = Expr::FunctionCall {
                    name: SmolStr::new("id"),
                    args: vec![Expr::Variable(ctx.current_alias.clone())],
                    distinct: false,
                };
                let predicate = predicate_to_prop_expr(pred, id_fn);
                ctx.current = Some(Operator::Filter {
                    predicate,
                    child: Box::new(child),
                });
            }

            GremlinStep::Where(inner_steps) | GremlinStep::Not(inner_steps) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let is_not = matches!(step, GremlinStep::Not(_));

                // Use predicate mode only when ALL inner steps are simple
                // predicate-convertible steps. Otherwise use traversal mode
                // (sub-traversal existence check via Apply + Count + Filter).
                // When there are multiple Select steps, they need chaining
                // (output of one feeds into the next), so use sub-traversal
                // mode instead of predicate mode.
                let select_count = inner_steps
                    .iter()
                    .filter(|s| matches!(s, GremlinStep::Select(..)))
                    .count();
                let can_be_predicate = !inner_steps.is_empty()
                    && select_count <= 1
                    && inner_steps.iter().all(|s| {
                        matches!(
                            s,
                            GremlinStep::Is(_)
                                | GremlinStep::Has(_, _)
                                | GremlinStep::HasPredicate(_, _)
                                | GremlinStep::HasLabel(_)
                                | GremlinStep::Values(_)
                                | GremlinStep::Label
                                | GremlinStep::Id
                                | GremlinStep::Select(..)
                        )
                    });

                if !can_be_predicate {
                    // Compile inner steps as a sub-traversal seeded with
                    // the ctx.current alias. Use Apply + Count + Filter to
                    // implement existence semantics.
                    // If the sub-traversal starts with as("label"), seed
                    // from that label's existing alias instead.
                    let mut sub = self.sub_compiler();
                    // Propagate map-entry context for unfolded traversals
                    if matches!(&child, Operator::Unwind { .. }) || self.seed_is_map_entry {
                        sub.seed_is_map_entry = true;
                    }
                    let (seed_alias, steps_slice) = match inner_steps.first() {
                        Some(GremlinStep::As(label)) if self.alias_map.contains_key(label) => {
                            (self.alias_map[label].clone(), &inner_steps[1..])
                        }
                        _ => (ctx.current_alias.clone(), &inner_steps[..]),
                    };

                    // Check if the sub-traversal ends with as("label")
                    // referencing an already-bound variable.  If so, the
                    // endpoint must be constrained to match that binding.
                    let endpoint_target =
                        if let Some(GremlinStep::As(label)) = steps_slice.last() {
                            self.alias_map.get(label).cloned()
                        } else {
                            None
                        };

                    let inner_op = sub.compile_seeded(steps_slice, &seed_alias)?;
                    // For not()/where() sub-traversals, only advance the
                    // alias counter (to avoid conflicts with outer scope).
                    // Do NOT merge alias_map — internal aliases from inside
                    // not()/where() must not pollute the outer scope, or
                    // downstream Project operators will attempt to reference
                    // those internal aliases on rows that don't carry them.
                    self.alias_counter = sub.alias_counter;
                    let sub_result_alias = sub.last_alias.clone();

                    // If the endpoint is constrained, add a filter so only
                    // traversers that land on the bound variable pass.
                    let inner_op = if let Some(target_alias) = endpoint_target {
                        Operator::Filter {
                            predicate: Expr::BinaryOp {
                                op: BinaryOp::Eq,
                                left: Box::new(Expr::Variable(sub_result_alias.clone())),
                                right: Box::new(Expr::Variable(target_alias)),
                            },
                            child: Box::new(inner_op),
                        }
                    } else {
                        inner_op
                    };

                    // Count the sub-traversal results
                    let cnt_alias = self.next_alias("wcnt");
                    let count_op = Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            cypher_core::planner::AggFunction::Count,
                            Expr::Variable(sub_result_alias),
                            None,
                            false,
                            cnt_alias.clone(),
                        )],
                        child: Box::new(inner_op),
                    };

                    // Wrap in Apply so each input row seeds the sub-traversal
                    let apply = Operator::Apply {
                        outer: Box::new(child),
                        inner: Box::new(count_op),
                    };

                    // Filter: keep rows where count > 0 (or == 0 for Not)
                    let predicate = if is_not {
                        Expr::BinaryOp {
                            op: BinaryOp::Eq,
                            left: Box::new(Expr::Variable(cnt_alias)),
                            right: Box::new(Expr::Integer(0)),
                        }
                    } else {
                        Expr::BinaryOp {
                            op: BinaryOp::Gt,
                            left: Box::new(Expr::Variable(cnt_alias)),
                            right: Box::new(Expr::Integer(0)),
                        }
                    };
                    ctx.current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(apply),
                    });
                } else {
                    let inner_pred = steps_to_predicate(
                        inner_steps,
                        &ctx.current_alias,
                        &self.alias_map,
                        &self.aggregate_keys,
                    );
                    let predicate = if is_not {
                        // Wrap in COALESCE so that Null (from cross-type
                        // comparison) becomes false before negation. This
                        // ensures not(has("name", P.gt(27))) correctly
                        // keeps traversers when the comparison is undefined.
                        let coalesced = Expr::FunctionCall {
                            name: SmolStr::new("coalesce"),
                            args: vec![inner_pred, Expr::Bool(false)],
                            distinct: false,
                        };
                        Expr::UnaryOp {
                            op: cypher_core::ast::UnaryOp::Not,
                            expr: Box::new(coalesced),
                        }
                    } else {
                        inner_pred
                    };
                    // Track traversal-ring targets for by() modulation on where().
                    if !is_not {
                        if let Some(targets) = extract_where_by_targets(
                            inner_steps,
                            &self.alias_map,
                            &ctx.current_alias,
                        ) {
                            ctx.pending_where_by_targets = targets;
                            ctx.pending_where_by_modulators.clear();
                        }
                    }
                    ctx.current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(child),
                    });
                }
            }

            GremlinStep::And(branches) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                if branches.is_empty() {
                    ctx.current = Some(child);
                } else {
                    ctx.current = Some(self.compile_connective_branches(
                        branches,
                        &ctx.current_alias,
                        child,
                        BinaryOp::And,
                    )?);
                }
            }

            GremlinStep::Or(branches) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                if branches.is_empty() {
                    ctx.current = Some(child);
                } else {
                    ctx.current = Some(self.compile_connective_branches(
                        branches,
                        &ctx.current_alias,
                        child,
                        BinaryOp::Or,
                    )?);
                }
            }

            GremlinStep::Is(predicate) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let pred_expr =
                    if let Predicate::IsType(GremlinValue::String(expected)) = predicate {
                        if let Some(ref subtype) = self.numeric_subtype {
                            if subtype == expected {
                                Expr::Bool(true)
                            } else {
                                Expr::Bool(false)
                            }
                        } else {
                            predicate_to_expr(predicate, &ctx.current_alias)
                        }
                    } else if let Some(label_expr) =
                        predicate_label_ref(predicate, &self.alias_map, &self.aggregate_keys)
                    {
                        let op = predicate_to_binary_op(predicate);
                        Expr::BinaryOp {
                            op,
                            left: Box::new(Expr::Variable(ctx.current_alias.clone())),
                            right: Box::new(label_expr),
                        }
                    } else if predicate_has_label_refs(
                        predicate,
                        &self.alias_map,
                        &self.aggregate_keys,
                    ) {
                        predicate_to_label_aware_expr(
                            predicate,
                            &Expr::Variable(ctx.current_alias.clone()),
                            &self.alias_map,
                            &self.aggregate_keys,
                        )
                    } else {
                        predicate_to_expr(predicate, &ctx.current_alias)
                    };
                ctx.current = Some(Operator::Filter {
                    predicate: pred_expr,
                    child: Box::new(child),
                });
            }

            GremlinStep::HasKey(keys) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                // After properties(), ctx.current alias is a property Map with ~key.
                // Check if ~key matches any of the given keys.
                let predicates: Vec<Expr> = keys
                    .iter()
                    .map(|k| {
                        // Check both: the element has the property (node/edge context)
                        // AND the property Map ~key matches (properties() context).
                        // Use ~key field for property Map objects.
                        Expr::BinaryOp {
                            op: BinaryOp::Eq,
                            left: Box::new(Expr::Property(
                                Box::new(Expr::Variable(ctx.current_alias.clone())),
                                SmolStr::new("~key"),
                            )),
                            right: Box::new(Expr::Str(k.as_str().into())),
                        }
                    })
                    .collect();
                // Any key matches (OR), or if checking on element, all must exist (AND).
                // In Gremlin, hasKey on properties filters for any of the keys.
                let predicate = if keys.len() == 1 {
                    // Single key: also support element context
                    Expr::BinaryOp {
                        op: BinaryOp::Or,
                        left: Box::new(predicates[0].clone()),
                        right: Box::new(Expr::IsNotNull(Box::new(Expr::Property(
                            Box::new(Expr::Variable(ctx.current_alias.clone())),
                            SmolStr::new(&keys[0]),
                        )))),
                    }
                } else {
                    predicates
                        .into_iter()
                        .reduce(|a, b| Expr::BinaryOp {
                            op: BinaryOp::Or,
                            left: Box::new(a),
                            right: Box::new(b),
                        })
                        .unwrap_or(Expr::Bool(true))
                };
                ctx.current = Some(Operator::Filter {
                    predicate,
                    child: Box::new(child),
                });
            }

            GremlinStep::HasValue(values) => {
                // hasValue filters by ~value (property context) or ctx.current value.
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                // Try ~value for property Map context, fall back to ctx.current alias.
                let tilde_val = Expr::Property(
                    Box::new(Expr::Variable(ctx.current_alias.clone())),
                    SmolStr::new("~value"),
                );
                if values.len() == 1 {
                    // Check ~value == val OR ctx.current == val
                    let val_expr = gremlin_value_to_expr(&values[0]);
                    let pred_tilde = Expr::BinaryOp {
                        op: BinaryOp::Eq,
                        left: Box::new(tilde_val),
                        right: Box::new(val_expr.clone()),
                    };
                    let pred_direct = Expr::BinaryOp {
                        op: BinaryOp::Eq,
                        left: Box::new(Expr::Variable(ctx.current_alias.clone())),
                        right: Box::new(val_expr),
                    };
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::BinaryOp {
                            op: BinaryOp::Or,
                            left: Box::new(pred_tilde),
                            right: Box::new(pred_direct),
                        },
                        child: Box::new(child),
                    });
                } else if !values.is_empty() {
                    // Filter null values out, then check ~value IN [non-null values]
                    // OR ~value IS NULL if null is in the list.
                    let non_null: Vec<Expr> = values
                        .iter()
                        .filter(|v| !matches!(v, GremlinValue::Null))
                        .map(gremlin_value_to_expr)
                        .collect();
                    let pred_in = if !non_null.is_empty() {
                        Some(Expr::BinaryOp {
                            op: BinaryOp::In,
                            left: Box::new(tilde_val.clone()),
                            right: Box::new(Expr::List(non_null)),
                        })
                    } else {
                        None
                    };
                    let predicate = match pred_in {
                        Some(pred) => pred,
                        None => Expr::Bool(false),
                    };
                    ctx.current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(child),
                    });
                } else {
                    ctx.current = Some(child);
                }
            }

            GremlinStep::HasValuePredicate(pred) => {
                // hasValue(P.gt(30)) — filter on ~value (property context) or ctx.current value.
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let tilde_val = Expr::Property(
                    Box::new(Expr::Variable(ctx.current_alias.clone())),
                    SmolStr::new("~value"),
                );
                let predicate = predicate_to_prop_expr(pred, tilde_val);
                ctx.current = Some(Operator::Filter {
                    predicate,
                    child: Box::new(child),
                });
            }

            GremlinStep::SimplePath => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                ctx.current = Some(Operator::GremlinSimplePath {
                    child: Box::new(child),
                    path_aliases: ctx.path_aliases.clone(),
                    by_keys: vec![],
                    from_label: None,
                    to_label: None,
                });
            }

            GremlinStep::CyclicPath => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                ctx.current = Some(Operator::GremlinCyclicPath {
                    child: Box::new(child),
                    path_aliases: ctx.path_aliases.clone(),
                    by_keys: vec![],
                    from_label: None,
                    to_label: None,
                });
                ctx.pending_path = true;
                ctx.path_by_keys.clear();
            }

            GremlinStep::Coin(probability) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let rand_expr = if let Some(seed) = self.seed {
                    // Seeded: use Java-compatible LCG for deterministic results
                    Expr::FunctionCall {
                        name: SmolStr::new("java_random_next_double"),
                        args: vec![Expr::Integer(seed)],
                        distinct: false,
                    }
                } else {
                    Expr::FunctionCall {
                        name: SmolStr::new("rand"),
                        args: vec![],
                        distinct: false,
                    }
                };
                ctx.current = Some(Operator::Filter {
                    predicate: Expr::BinaryOp {
                        op: BinaryOp::Lt,
                        left: Box::new(rand_expr),
                        right: Box::new(Expr::Float(*probability)),
                    },
                    child: Box::new(child),
                });
            }

            GremlinStep::FilterTraversal(filter_steps) => {
                // filter(traversal) — keep traversers where sub-traversal is true
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                // Try to extract a predicate from the inner steps
                let pred = steps_to_predicate(
                    filter_steps,
                    &ctx.current_alias,
                    &self.alias_map,
                    &self.aggregate_keys,
                );
                // If steps_to_predicate returned true (couldn't convert) and
                // there are actual steps, fall back to sub-traversal existence
                // check via Apply + Count + Filter (same pattern as Where).
                if pred == Expr::Bool(true) && !filter_steps.is_empty() {
                    let mut sub = self.sub_compiler();
                    // If the child is an Unwind, the seed element is a map
                    // entry — propagate this so select(Column.values) in
                    // the sub-traversal extracts the value directly.
                    if matches!(&child, Operator::Unwind { .. }) || self.seed_is_map_entry {
                        sub.seed_is_map_entry = true;
                    }
                    let inner_op = sub.compile_seeded(filter_steps, &ctx.current_alias)?;
                    // filter(__...) is a scoped existence check. Labels and
                    // aliases created only for the filter body must not
                    // leak back into the outer traversal, or later
                    // select(Pop...) steps can read those internal aliases
                    // as if they were outer traverser bindings.
                    self.alias_counter = sub.alias_counter;
                    let sub_result_alias = sub.last_alias.clone();
                    let cnt_alias = self.next_alias("fcnt");
                    let count_op = Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            cypher_core::planner::AggFunction::Count,
                            Expr::Variable(sub_result_alias),
                            None,
                            false,
                            cnt_alias.clone(),
                        )],
                        child: Box::new(inner_op),
                    };
                    let apply = Operator::Apply {
                        outer: Box::new(child),
                        inner: Box::new(count_op),
                    };
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::BinaryOp {
                            op: BinaryOp::Gt,
                            left: Box::new(Expr::Variable(cnt_alias)),
                            right: Box::new(Expr::Integer(0)),
                        },
                        child: Box::new(apply),
                    });
                } else {
                    ctx.current = Some(Operator::Filter {
                        predicate: pred,
                        child: Box::new(child),
                    });
                }
            }

            GremlinStep::All(pred) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let (pred_op_str, pred_val_expr) = predicate_to_args(pred);
                let pred_fn = Expr::FunctionCall {
                    name: SmolStr::new("gremlin_all"),
                    args: vec![
                        Expr::Variable(ctx.current_alias.clone()),
                        Expr::Str(pred_op_str),
                        pred_val_expr,
                    ],
                    distinct: false,
                };
                ctx.current = Some(Operator::Filter {
                    predicate: pred_fn,
                    child: Box::new(child),
                });
            }

            GremlinStep::Any(pred) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let (pred_op_str, pred_val_expr) = predicate_to_args(pred);
                let pred_fn = Expr::FunctionCall {
                    name: SmolStr::new("gremlin_any"),
                    args: vec![
                        Expr::Variable(ctx.current_alias.clone()),
                        Expr::Str(pred_op_str),
                        pred_val_expr,
                    ],
                    distinct: false,
                };
                ctx.current = Some(Operator::Filter {
                    predicate: pred_fn,
                    child: Box::new(child),
                });
            }

            GremlinStep::NonePred(pred) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let (pred_op_str, pred_val_expr) = predicate_to_args(pred);
                let pred_fn = Expr::FunctionCall {
                    name: SmolStr::new("gremlin_none"),
                    args: vec![
                        Expr::Variable(ctx.current_alias.clone()),
                        Expr::Str(pred_op_str),
                        pred_val_expr,
                    ],
                    distinct: false,
                };
                ctx.current = Some(Operator::Filter {
                    predicate: pred_fn,
                    child: Box::new(child),
                });
            }

            // Predicate steps used inside Where/Is — should not appear at top level
            GremlinStep::Eq(_)
            | GremlinStep::Neq(_)
            | GremlinStep::Gt(_)
            | GremlinStep::Gte(_)
            | GremlinStep::Lt(_)
            | GremlinStep::Lte(_)
            | GremlinStep::Within(_)
            | GremlinStep::Without(_) => {
                return Err(CompileError::InvalidSequence(
                    "predicate steps should not appear at the top level".into(),
                ));
            }

            // Unsupported steps — parsed successfully but not compilable
            _ => unreachable!(
                "compile_filter_step received non-filter variant: {:?}",
                step
            ),
        }
        #[allow(unreachable_code)]
        Ok(StepFlow::Proceed)
    }
}
