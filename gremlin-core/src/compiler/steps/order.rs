//! `order` category step compilation arms extracted from `dispatch.rs`.
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
    pub(super) fn compile_order_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::Order => {
                ctx.pending_order = true;
                // Actual Sort operator created when we see By (or at the end)
            }

            GremlinStep::ByTraversal(sub_steps, _descending) => {
                // by(traversal) after valueMap — unfold list values
                if ctx.pending_valuemap && !ctx.valuemap_by_consumed {
                    let is_unfold =
                        sub_steps.len() == 1 && matches!(sub_steps[0], GremlinStep::Unfold);
                    if is_unfold {
                        // Wrap the ctx.current valueMap result to unfold lists
                        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                        let alias = self.next_alias("vmuf");
                        let mut items = vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_valuemap_unfold"),
                                args: vec![Expr::Variable(ctx.current_alias.clone())],
                                distinct: false,
                            },
                            Some(alias.clone()),
                        )];
                        items.extend(self.label_carry_items());
                        ctx.current = Some(Operator::Project {
                            items,
                            child: Box::new(child),
                        });
                        ctx.current_alias = alias;
                    }
                    ctx.valuemap_by_consumed = true;
                } else if ctx.pending_valuemap && ctx.valuemap_by_consumed {
                    return Err(CompileError::InvalidSequence(
                        "step can only have one by modulator".into(),
                    ));
                } else if ctx.pending_project {
                    // by(traversal) after project() — compile the sub-traversal
                    // and convert to GremlinProject if not already done.
                    if ctx.project_by_count < ctx.project_keys.len() {
                        // Compile the sub-traversal seeded with the pre-project alias
                        let mut sub = self.sub_compiler();
                        if self.seed_is_map_entry {
                            sub.seed_is_map_entry = true;
                        }
                        let sub_op = sub.compile_seeded(sub_steps, &ctx.pre_project_alias)?;
                        let sub_out_alias = sub.last_alias.clone();
                        self.absorb(&sub);

                        // Convert Operator::Project → GremlinProject on first ByTraversal
                        match &ctx.current {
                            Some(Operator::Project { items, child }) => {
                                // Build GremlinProject entries from existing items
                                let mut entries: Vec<(String, Operator, Identifier, bool)> =
                                    Vec::new();
                                for (i, (_expr, alias_opt)) in items.iter().enumerate() {
                                    if i < ctx.project_keys.len() {
                                        if i == ctx.project_by_count {
                                            // This is the by(traversal) entry — not identity
                                            entries.push((
                                                ctx.project_keys[i].clone(),
                                                sub_op.clone(),
                                                sub_out_alias.clone(),
                                                false,
                                            ));
                                        } else if i < ctx.project_by_count {
                                            // Previously filled by By() — check if identity.
                                            // If items[i].0 is Expr::Variable it was empty by().
                                            let is_ident = matches!(&items[i].0, Expr::Variable(_));
                                            let prop_op = Operator::Project {
                                                items: vec![(
                                                    items[i].0.clone(),
                                                    alias_opt.clone(),
                                                )],
                                                child: Box::new(Operator::SingleRow),
                                            };
                                            entries.push((
                                                ctx.project_keys[i].clone(),
                                                prop_op,
                                                alias_opt.as_ref().cloned().unwrap_or_else(|| {
                                                    Identifier::new(&ctx.project_keys[i])
                                                }),
                                                is_ident,
                                            ));
                                        } else {
                                            // Not yet filled — default to property access
                                            let prop_op = Operator::Project {
                                                items: vec![(
                                                    items[i].0.clone(),
                                                    alias_opt.clone(),
                                                )],
                                                child: Box::new(Operator::SingleRow),
                                            };
                                            entries.push((
                                                ctx.project_keys[i].clone(),
                                                prop_op,
                                                alias_opt.as_ref().cloned().unwrap_or_else(|| {
                                                    Identifier::new(&ctx.project_keys[i])
                                                }),
                                                false,
                                            ));
                                        }
                                    }
                                }
                                let proj_alias = self.next_alias("proj");
                                ctx.current = Some(Operator::GremlinProject {
                                    entries,
                                    child: child.clone(),
                                    current_alias: ctx.pre_project_alias.clone(),
                                    out_alias: proj_alias,
                                    productive: self.productive_by,
                                });
                            }
                            Some(Operator::GremlinProject {
                                entries: old_entries,
                                child,
                                current_alias: ca,
                                out_alias,
                                ..
                            }) => {
                                let mut entries = old_entries.clone();
                                if ctx.project_by_count < entries.len() {
                                    entries[ctx.project_by_count] = (
                                        ctx.project_keys[ctx.project_by_count].clone(),
                                        sub_op,
                                        sub_out_alias,
                                        false, // by(traversal) — not identity
                                    );
                                }
                                ctx.current = Some(Operator::GremlinProject {
                                    entries,
                                    child: child.clone(),
                                    current_alias: ca.clone(),
                                    out_alias: out_alias.clone(),
                                    productive: self.productive_by,
                                });
                            }
                            _ => {}
                        }
                        ctx.project_by_count += 1;
                        if ctx.project_by_count >= ctx.project_keys.len() {
                            ctx.pending_project = false;
                            if let Some(Operator::GremlinProject {
                                out_alias: proj_alias,
                                ..
                            }) = &ctx.current
                            {
                                ctx.current_alias = proj_alias.clone();
                            }
                        }
                    }
                }
                // Non-project contexts: delegate ByTraversal to the same
                // behavior as By by reconstructing the original key string.
                if !ctx.pending_project {
                    if ctx.pending_order {
                        // by(traversal) after order() — compile sub-traversal for sort
                        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                        let mut sub = self.sub_compiler();
                        let sub_op = sub.compile_seeded(sub_steps, &ctx.current_alias)?;
                        let sub_out_alias = sub.last_alias.clone();
                        self.absorb(&sub);
                        let applied = Operator::Apply {
                            outer: Box::new(child),
                            inner: Box::new(sub_op),
                        };
                        ctx.current = Some(Operator::Sort {
                            items: vec![(Expr::Variable(sub_out_alias), !_descending)],
                            gremlin_order: true,
                            child: Box::new(applied),
                        });
                        ctx.pending_order = false;
                    } else if ctx.pending_select {
                        // by(traversal) after select() — preserve the
                        // real traversal for per-key execution.
                        ctx.select_by_mods
                            .push(SelectByModulator::Traversal(sub_steps.to_vec()));
                    } else if ctx.pending_path {
                        // by(traversal) after path() — extract property key
                        // from patterns like by(values('name').toUpper())
                        let key = extract_by_traversal_key(sub_steps);
                        ctx.path_by_keys.push(key);
                    } else if ctx.pending_dedup {
                        // by(traversal) after dedup() — compile sub-traversal for dedup key
                        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                        let mut sub = self.sub_compiler();
                        let sub_op = sub.compile_seeded(sub_steps, &ctx.current_alias)?;
                        let sub_out_alias = sub.last_alias.clone();
                        self.absorb(&sub);
                        let applied = Operator::Apply {
                            outer: Box::new(child),
                            inner: Box::new(sub_op),
                        };
                        ctx.current = Some(Operator::Distinct {
                            child: Box::new(applied),
                            by_key: Some(sub_out_alias.to_string()),
                            productive: self.productive_by,
                        });
                        ctx.pending_dedup = false;
                    } else if !ctx.pending_where_by_targets.is_empty() {
                        ctx.pending_where_by_modulators
                            .push(extract_by_traversal_key(sub_steps));
                    } else if ctx.pending_group {
                        // by(traversal) after group() — compile sub-traversal for key or value
                        if ctx.pending_group_by_count == 0 {
                            // First by(traversal) = group key — traversal is not identity
                            if let Some(Operator::GremlinGroup {
                                ref mut key_is_identity,
                                ..
                            }) = ctx.current
                            {
                                *key_is_identity = false;
                            }
                            let mut sub = self.sub_compiler();
                            let sub_op = sub.compile_seeded(sub_steps, &ctx.pre_group_alias)?;
                            let sub_out_alias = sub.last_alias.clone();
                            self.absorb(&sub);
                            if let Some(Operator::GremlinGroup {
                                ref mut key_traversal,
                                ..
                            }) = ctx.current
                            {
                                *key_traversal = Some((Box::new(sub_op), sub_out_alias));
                            }
                            ctx.pending_group_by_count = 1;
                        } else {
                            // Second by(traversal) = value aggregation
                            // Check for trailing agg step (count/sum/fold)
                            let (proj_steps, trailing_agg) =
                                split_trailing_group_value_agg(sub_steps);

                            if proj_steps.is_empty() {
                                if let Some(Operator::GremlinGroup {
                                    ref mut agg_fn,
                                    ref mut explicit_fold,
                                    ..
                                }) = ctx.current
                                {
                                    if let Some(agg) = trailing_agg {
                                        if matches!(agg, AggFunction::Collect) {
                                            *explicit_fold = true;
                                        }
                                        *agg_fn = agg;
                                    }
                                }
                            } else {
                                let mut sub = self.sub_compiler();
                                // If the element going into group() is a single-entry Map
                                // from unfold() (i.e., a Map.Entry), mark the sub-compiler
                                // so that select(Column.keys/values) uses map-entry semantics
                                // (returning the key/value directly, not a collection).
                                let outer_child_is_unwind = if let Some(Operator::GremlinGroup {
                                    child,
                                    ..
                                }) = &ctx.current
                                {
                                    matches!(child.as_ref(), Operator::Unwind { .. })
                                } else {
                                    false
                                };
                                sub.seed_is_map_entry = outer_child_is_unwind;
                                let sub_op =
                                    sub.compile_seeded(proj_steps, &ctx.pre_group_alias)?;
                                let sub_out_alias = sub.last_alias.clone();
                                self.absorb(&sub);
                                if let Some(Operator::GremlinGroup {
                                    ref mut value_traversal,
                                    ref mut agg_fn,
                                    ref mut explicit_fold,
                                    ..
                                }) = ctx.current
                                {
                                    *value_traversal = Some((Box::new(sub_op), sub_out_alias));
                                    if let Some(agg) = trailing_agg {
                                        if matches!(agg, AggFunction::Collect) {
                                            *explicit_fold = true;
                                        }
                                        *agg_fn = agg;
                                    }
                                }
                            }
                            ctx.pending_group = false;
                        }
                    } else if ctx.pending_aggregate {
                        // by(traversal) after aggregate() — compile sub-traversal
                        let mut sub = self.sub_compiler();
                        let sub_op = sub.compile_seeded(sub_steps, &ctx.current_alias)?;
                        let sub_out_alias = sub.last_alias.clone();
                        self.absorb(&sub);
                        if let Some(Operator::GremlinAggregate {
                            ref mut by_traversal,
                            ..
                        }) = ctx.current
                        {
                            *by_traversal = Some((Box::new(sub_op), sub_out_alias));
                        }
                        ctx.pending_aggregate = false;
                        ctx.aggregate_by_consumed = true;
                    } else if ctx.pending_format {
                        // by(traversal) after format() — compile sub-traversal for by-modulation
                        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                        let mut sub = self.sub_compiler();
                        let sub_op = sub.compile_seeded(sub_steps, &ctx.format_pre_alias)?;
                        let sub_out_alias = sub.last_alias.clone();
                        self.absorb(&sub);
                        // Limit to 1 result per input row
                        let limited = Operator::Limit {
                            count: Expr::Integer(1),
                            child: Box::new(sub_op),
                        };
                        ctx.current = Some(Operator::Apply {
                            outer: Box::new(child),
                            inner: Box::new(limited),
                        });
                        ctx.format_by_aliases.push(sub_out_alias);
                    } else if ctx.pending_sack_op && !ctx.pending_sack_by_consumed {
                        // by(traversal) after sack(op) — rewrite sack assignment
                        // to use the sub-traversal result instead of ctx.current_alias.
                        let mut sub = self.sub_compiler();
                        let sub_op = sub.compile_seeded(sub_steps, &ctx.current_alias)?;
                        let sub_out_alias = sub.last_alias.clone();
                        self.absorb(&sub);
                        let by_expr = Expr::Variable(sub_out_alias);
                        // Wrap ctx.current operator in Apply so the sub-traversal runs
                        // per-row, then rewrite the sack Project's rhs.
                        if let Some(ref mut proj) = ctx.current {
                            // First, wrap child in Apply with the sub-traversal
                            let sack_var = Identifier::new("__sack");
                            if let Operator::Project {
                                ref mut items,
                                ref mut child,
                            } = proj
                            {
                                **child = Operator::Apply {
                                    outer: child.clone(),
                                    inner: Box::new(sub_op),
                                };
                                if let Some((expr, _)) = items.first_mut() {
                                    rewrite_sack_by(expr, &ctx.current_alias, &by_expr);
                                }
                            }
                            // Filter out rows where __sack is null
                            let proj_taken = std::mem::replace(proj, Operator::SingleRow);
                            *proj = Operator::Filter {
                                predicate: Expr::IsNotNull(Box::new(Expr::Variable(sack_var))),
                                child: Box::new(proj_taken),
                            };
                        }
                        ctx.pending_sack_op = false;
                        ctx.pending_sack_by_consumed = true;
                    }
                }
            }

            _ => unreachable!("compile_order_step received non-order variant: {:?}", step),
        }
        #[allow(unreachable_code)]
        Ok(StepFlow::Proceed)
    }
}
