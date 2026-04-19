//! `branch` category step compilation arms extracted from `dispatch.rs`.
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
    pub(super) fn compile_branch_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::Repeat(label, body_steps) => {
                // Validate: inject() is not allowed inside repeat()
                if body_steps
                    .iter()
                    .any(|s| matches!(s, GremlinStep::Inject(_)))
                {
                    return Err(CompileError::InvalidSequence(
                        "The parent of inject()-step can not be repeat()-step".into(),
                    ));
                }
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let seed_alias = ctx.current_alias.clone();

                // Pre-scan: when Pop-select is downstream, check if the
                // body re-binds any existing label.  Create shadow aliases
                // so the executor can preserve original bindings for
                // Pop.first.
                let mut pop_origins: Vec<(String, String)> = Vec::new();
                if self.needs_label_stacks {
                    for step in body_steps.iter() {
                        if let GremlinStep::As(lbl) = step {
                            if let Some(orig) = self.alias_map.get(lbl).cloned() {
                                let shadow = self.next_alias("pop_orig");
                                self.label_stacks
                                    .entry(lbl.clone())
                                    .or_default()
                                    .insert(0, shadow.clone());
                                pop_origins
                                    .push((shadow.as_str().to_string(), orig.as_str().to_string()));
                            }
                        }
                    }
                }

                let mut sub = self.sub_compiler();
                if self.seed_is_map_entry {
                    sub.seed_is_map_entry = true;
                }
                let body = sub.compile_seeded(body_steps, &ctx.current_alias)?;
                let body_out_alias = sub.last_alias.clone();
                // Collect intermediate body path aliases (between seed and
                // body-output) so the executor can accumulate edge values
                // in __gremlin_path_acc for patterns like outE().inV().
                let body_pa: Vec<String> = if self.needs_path_tracking {
                    sub.path_tracking
                        .iter()
                        .filter(|id| {
                            id.as_str() != seed_alias.as_str()
                                && id.as_str() != body_out_alias.as_str()
                        })
                        .map(|id| id.as_str().to_string())
                        .collect()
                } else {
                    vec![]
                };
                self.absorb(&sub);
                // Apply any pending emit/until that appeared before the repeat
                let emit_val = ctx.pending_emit;
                ctx.pending_emit = false;
                let until_val = if let Some(until_steps) = ctx.pending_until.take() {
                    let mut sub2 = self.sub_compiler();
                    let until_op = sub2.compile_seeded(&until_steps, &ctx.current_alias)?;
                    self.absorb(&sub2);
                    Some(Box::new(until_op))
                } else {
                    None
                };
                // If body produces a different alias, remap it back to the seed
                let remap = if body_out_alias.as_str() != seed_alias.as_str() {
                    Some((
                        body_out_alias.as_str().to_string(),
                        seed_alias.as_str().to_string(),
                    ))
                } else {
                    None
                };
                let times_val = ctx.pending_times.take();
                let times_before_val = times_val.is_some();
                ctx.current = Some(Operator::Repeat {
                    body: Box::new(body),
                    times: times_val,
                    until: until_val,
                    emit: emit_val,
                    emit_before: emit_val,
                    emit_predicate: if let Some(ep_steps) = ctx.pending_emit_pred.take() {
                        let mut sub3 = self.sub_compiler();
                        let ep_op = sub3.compile_seeded(&ep_steps, &ctx.current_alias)?;
                        self.absorb(&sub3);
                        Some(Box::new(ep_op))
                    } else {
                        None
                    },
                    remap,
                    times_before: times_before_val,
                    loop_label: label.as_deref().map(Box::from),
                    body_path_aliases: body_pa,
                    pop_origins,
                    needs_path: self.needs_path_tracking,
                    child: Box::new(child),
                });
            }

            GremlinStep::Times(n) => {
                // Modify preceding Repeat
                if let Some(Operator::Repeat {
                    ref mut times,
                    ref mut times_before,
                    ..
                }) = ctx.current
                {
                    *times = Some(*n as usize);
                    *times_before = false; // times() after repeat() → do-while
                } else {
                    // times() before repeat() — store as pending
                    ctx.pending_times = Some(*n as usize);
                }
            }

            GremlinStep::Until(until_steps) => {
                if let Some(Operator::Repeat { ref mut until, .. }) = ctx.current {
                    // Modify preceding Repeat
                    let mut sub = self.sub_compiler();
                    let until_op = sub.compile_seeded(until_steps, &ctx.current_alias)?;
                    self.absorb(&sub);
                    *until = Some(Box::new(until_op));
                } else {
                    // until() before repeat() — store as pending
                    ctx.pending_until = Some(until_steps.clone());
                }
            }

            GremlinStep::Emit => {
                if let Some(Operator::Repeat {
                    ref mut emit,
                    ref mut emit_before,
                    ..
                }) = ctx.current
                {
                    *emit = true;
                    // emit() after repeat() → post-emit (don't include starting traversers)
                    *emit_before = false;
                } else {
                    // emit() before repeat() — store as pending
                    ctx.pending_emit = true;
                }
            }

            GremlinStep::EmitPred(pred_steps) => {
                if let Some(Operator::Repeat {
                    ref mut emit,
                    ref mut emit_before,
                    ref mut emit_predicate,
                    ..
                }) = ctx.current
                {
                    *emit = true;
                    *emit_before = false;
                    let mut sub = self.sub_compiler();
                    let ep_op = sub.compile_seeded(pred_steps, &ctx.current_alias)?;
                    self.absorb(&sub);
                    *emit_predicate = Some(Box::new(ep_op));
                } else {
                    // emit(pred) before repeat() — store as pending
                    ctx.pending_emit = true;
                    ctx.pending_emit_pred = Some(pred_steps.clone());
                }
            }

            GremlinStep::Choose(pred_steps, true_steps, false_steps) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let mut sub = self.sub_compiler();
                if true_steps.is_empty() && false_steps.is_none() {
                    // 1-arg choose: value-based dispatch, options come from Option steps
                    let selector = sub.compile_seeded(pred_steps, &ctx.current_alias)?;
                    let sel_alias = sub.last_alias.clone();
                    self.absorb(&sub);
                    let choose_alias = self.next_alias("chm");
                    ctx.current = Some(Operator::ChooseMap {
                        selector: Box::new(selector),
                        selector_alias: sel_alias,
                        options: Vec::new(),
                        child: Box::new(child),
                        output_alias: Some(choose_alias.clone()),
                    });
                    // Defer alias switch until after all option() steps
                    ctx.pending_choose_output = Some(choose_alias);
                } else {
                    let pred_op = sub.compile_seeded(pred_steps, &ctx.current_alias)?;
                    let true_op = sub.compile_seeded(true_steps, &ctx.current_alias)?;
                    let t_alias = sub.last_alias.clone();
                    let false_op = match false_steps {
                        Some(steps) => {
                            Some(Box::new(sub.compile_seeded(steps, &ctx.current_alias)?))
                        }
                        None => None,
                    };
                    // For false branch: use the sub-compiler's alias if a false
                    // branch exists, otherwise use the seed alias (ctx.current_alias
                    // before choose) for pass-through rows.
                    let f_alias = if false_steps.is_some() {
                        sub.last_alias.clone()
                    } else {
                        ctx.current_alias.clone()
                    };
                    self.absorb(&sub);

                    // Create a unified output alias for choose branches.
                    let choose_alias = self.next_alias("ch");
                    ctx.current_alias = choose_alias.clone();

                    ctx.current = Some(Operator::Choose {
                        predicate: Box::new(pred_op),
                        if_true: Box::new(true_op),
                        if_false: false_op,
                        child: Box::new(child),
                        true_alias: Some(t_alias),
                        false_alias: Some(f_alias),
                        output_alias: Some(choose_alias.clone()),
                    });
                }
            }

            GremlinStep::Coalesce(alternatives) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                // Compile each alternative and track their output aliases.
                let mut alt_ops = Vec::new();
                let mut alt_aliases = Vec::new();
                for s in alternatives {
                    let mut sub = self.sub_compiler();
                    let op = sub.compile_seeded(s, &ctx.current_alias)?;
                    alt_aliases.push(sub.last_alias.clone());
                    // Propagate edge source so otherV() after coalesce works.
                    if sub.edge_src_vertex.is_some() && self.edge_src_vertex.is_none() {
                        self.edge_src_vertex = sub.edge_src_vertex.clone();
                    }
                    self.absorb(&sub);
                    alt_ops.push(op);
                }

                // Normalize: wrap each alternative in a Project to rename
                // its output to a common alias.
                let coal_alias = self.next_alias("coal");
                let edge_src = self.edge_src_vertex.clone();
                let normalized: Vec<Operator> = alt_ops
                    .into_iter()
                    .zip(alt_aliases.iter())
                    .map(|(op, alt_alias)| {
                        if alt_alias.as_str() == coal_alias.as_str() {
                            op
                        } else {
                            let mut items = vec![
                                (Expr::Variable(alt_alias.clone()), Some(coal_alias.clone())),
                                // Also carry forward the original alias for path()
                                (Expr::Variable(alt_alias.clone()), Some(alt_alias.clone())),
                            ];
                            // Carry the edge source vertex so otherV() can
                            // find it in the row after coalesce.
                            if let Some(ref src) = edge_src {
                                items.push((Expr::Variable(src.clone()), Some(src.clone())));
                            }
                            Operator::Project {
                                items,
                                child: Box::new(op),
                            }
                        }
                    })
                    .collect();

                ctx.current = Some(Operator::Coalesce {
                    alternatives: normalized,
                    child: Box::new(child),
                });
                ctx.current_alias = coal_alias;
                ctx.path_aliases.push(ctx.current_alias.clone());
                self.path_tracking.push(ctx.current_alias.clone());
            }

            GremlinStep::Union(branches) => {
                let child = ctx.current.take().unwrap_or(Operator::SingleRow);
                if branches.is_empty() {
                    // union() with no branches → produce no results
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::Bool(false),
                        child: Box::new(child),
                    });
                } else {
                    let all_branches_have_source = branches
                        .iter()
                        .all(|s| Self::steps_have_independent_source(s));

                    // Compile each branch and track its result alias.
                    let mut branch_ops: Vec<Operator> = Vec::new();
                    let mut branch_aliases: Vec<Identifier> = Vec::new();
                    let mut branch_path_trackings: Vec<Vec<Identifier>> = Vec::new();
                    for s in branches {
                        let mut sub = self.sub_compiler();
                        let has_source = Self::steps_have_independent_source(s);
                        let op = if has_source {
                            sub.compile_seeded(s, &ctx.current_alias)?
                        } else {
                            sub.compile_from(
                                s,
                                Some(child.clone()),
                                ctx.current_alias.clone(),
                                ctx.path_aliases.clone(),
                            )?
                        };
                        branch_aliases.push(sub.last_alias.clone());
                        branch_path_trackings.push(sub.path_tracking.clone());
                        self.absorb(&sub);
                        branch_ops.push(op);
                    }

                    // Choose a common output alias for the union.
                    let union_alias = self.next_alias("u");

                    // Wrap each branch in a Project to normalize the output alias.
                    let normalized: Vec<Operator> = branch_ops
                        .into_iter()
                        .zip(branch_aliases.iter())
                        .zip(branch_path_trackings.iter())
                        .map(|((op, src_alias), branch_paths)| {
                            if src_alias == &union_alias {
                                op
                            } else {
                                let mut items = vec![(
                                    Expr::Variable(src_alias.clone()),
                                    Some(union_alias.clone()),
                                )];
                                // Carry forward all step labels
                                items.extend(self.label_carry_items());

                                // When path tracking is active, build
                                // __gremlin_path_acc from the branch's
                                // intermediate path aliases (all but the
                                // last, which becomes the union output).
                                if self.needs_path_tracking && branch_paths.len() > 1 {
                                    let acc_items: Vec<Expr> = branch_paths
                                        [..branch_paths.len() - 1]
                                        .iter()
                                        .map(|a| Expr::Variable(a.clone()))
                                        .collect();
                                    items.push((
                                        Expr::List(acc_items),
                                        Some(Identifier::new("__gremlin_path_acc")),
                                    ));
                                }

                                Operator::Project {
                                    items,
                                    child: Box::new(op),
                                }
                            }
                        })
                        .collect();

                    // Build a left-associative union chain
                    let union = normalized
                        .into_iter()
                        .reduce(|left, right| Operator::Union {
                            left: Box::new(left),
                            right: Box::new(right),
                            all: true,
                        })
                        .unwrap_or(Operator::SingleRow);

                    // Gremlin union() operates over the ctx.current traverser
                    // stream. Even when that stream is provided through
                    // SingleRow/foreach_seed (repeat(), choose(), local()),
                    // wrapping the union in Apply would incorrectly force
                    // per-row semantics and break barrier steps like
                    // limit(), count(), and barrier() inside branches.
                    let _ = all_branches_have_source;
                    ctx.current = Some(union);
                    ctx.current_alias = union_alias;
                    ctx.path_aliases.push(ctx.current_alias.clone());
                    self.path_tracking.push(ctx.current_alias.clone());
                }
            }

            GremlinStep::Match(patterns) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let mut compiled_patterns = Vec::with_capacity(patterns.len());
                let mut all_alias_pairs: Vec<(String, Identifier)> = Vec::new();
                let mut pattern_labels_list: Vec<Vec<(String, Identifier)>> = Vec::new();
                // Separate filter patterns (not/where) from join patterns.
                // Filter patterns are applied as post-filters after the match join.
                let mut filter_pattern_steps: Vec<Vec<GremlinStep>> = Vec::new();
                // Track whether the input traverser has been bound to a
                // label. Only the first new label gets the input; subsequent
                // new labels enumerate via V(None).
                let mut input_bound = false;
                // Flatten and() patterns: match(a, and(b, c)) → match(a, b, c)
                let mut flat_patterns: Vec<Vec<GremlinStep>> = Vec::new();
                fn flatten_match_patterns(
                    patterns: &[Vec<GremlinStep>],
                    out: &mut Vec<Vec<GremlinStep>>,
                ) {
                    for p in patterns {
                        if p.len() == 1 {
                            if let Some(GremlinStep::And(branches)) = p.first() {
                                flatten_match_patterns(branches, out);
                                continue;
                            }
                        }
                        out.push(p.clone());
                    }
                }
                flatten_match_patterns(patterns, &mut flat_patterns);

                for pattern_steps in &flat_patterns {
                    // Detect filter-only patterns: a single Not or Where step
                    let is_filter = pattern_steps.len() == 1
                        && matches!(
                            pattern_steps.first(),
                            Some(GremlinStep::Not(_)) | Some(GremlinStep::Where(_))
                        );
                    if is_filter {
                        filter_pattern_steps.push(pattern_steps.clone());
                        continue;
                    }

                    let mut sub = self.sub_compiler();

                    // Check if the pattern starts with As("X").
                    let first_as_label = pattern_steps.first().and_then(|s| {
                        if let GremlinStep::As(label) = s {
                            Some(label.clone())
                        } else {
                            None
                        }
                    });

                    // Track whether the seed label is already bound
                    let seed_already_bound = first_as_label
                        .as_ref()
                        .map(|l| sub.alias_map.contains_key(l))
                        .unwrap_or(false);

                    let compiled = if let Some(ref label) = first_as_label {
                        if let Some(existing_alias) = sub.alias_map.get(label).cloned() {
                            // Label already bound — use existing alias as seed.
                            let remaining = &pattern_steps[1..];
                            sub.compile_with_seed(remaining, existing_alias)?
                        } else if !input_bound {
                            // First new label — bind to the ctx.current traverser.
                            input_bound = true;
                            sub.alias_map
                                .insert(label.clone(), ctx.current_alias.clone());
                            sub.compile_with_seed(&pattern_steps[1..], ctx.current_alias.clone())?
                        } else {
                            // Subsequent new label — create an explicit
                            // NodeScan with a known alias so the label maps
                            // to the correct column.
                            let scan_alias = sub.next_alias("v");
                            sub.alias_map.insert(label.clone(), scan_alias.clone());
                            let scan = Operator::NodeScan {
                                label: None,
                                alias: scan_alias.clone(),
                            };
                            sub.compile_from(
                                &pattern_steps[1..],
                                Some(scan),
                                scan_alias.clone(),
                                vec![scan_alias],
                            )?
                        }
                    } else {
                        // No leading as() — seed from the ctx.current traverser.
                        sub.compile_with_seed(pattern_steps, ctx.current_alias.clone())?
                    };

                    // Determine per-pattern produced labels: all labels
                    // EXCEPT the seed label when it was already bound AND
                    // its alias collides with another label (Repeat remap).
                    let produced: Vec<(String, Identifier)> = sub
                        .alias_map
                        .iter()
                        .filter(|(label, alias)| {
                            if seed_already_bound && first_as_label.as_ref() == Some(label) {
                                // If the seed alias is shared with another
                                // label, exclude seed — its value may be
                                // overwritten by the pattern (e.g. Repeat).
                                let shared = sub
                                    .alias_map
                                    .iter()
                                    .any(|(l, a)| l != *label && a.as_str() == alias.as_str());
                                !shared
                            } else {
                                true
                            }
                        })
                        .map(|(l, a)| (l.clone(), a.clone()))
                        .collect();
                    pattern_labels_list.push(produced);

                    for (k, v) in &sub.alias_map {
                        self.alias_map.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                    // Collect ALL alias→label pairs from each pattern
                    // (different patterns may use different aliases for the
                    // same label).
                    for (label, alias) in &sub.alias_map {
                        all_alias_pairs.push((label.clone(), alias.clone()));
                    }
                    self.absorb(&sub);
                    compiled_patterns.push(compiled);
                }
                // Build the label→alias mapping with ALL aliases.
                let label_map: Vec<(String, Identifier)> = all_alias_pairs;
                let match_result_alias = self.next_alias("match");
                ctx.current = Some(Operator::GremlinMatch {
                    patterns: compiled_patterns,
                    pattern_labels: pattern_labels_list,
                    label_map: label_map.clone(),
                    result_alias: match_result_alias.clone(),
                    child: Box::new(child),
                });
                ctx.current_alias = match_result_alias;

                // Apply not()/where() filter patterns as post-filters on
                // the match results. Each filter is compiled as a sub-
                // traversal existence check using Apply + Count + Filter.
                for fsteps in &filter_pattern_steps {
                    let (is_not, inner_steps) = match fsteps.first() {
                        Some(GremlinStep::Not(inner)) => (true, inner.as_slice()),
                        Some(GremlinStep::Where(inner)) => (false, inner.as_slice()),
                        _ => continue,
                    };
                    let match_child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    // Determine seed alias from the first As() in the inner steps.
                    let mut sub = self.sub_compiler();
                    // Inject match label→alias bindings into sub compiler
                    for (label, alias) in &label_map {
                        sub.alias_map
                            .entry(label.clone())
                            .or_insert_with(|| alias.clone());
                    }
                    let (seed_alias, compile_steps) = match inner_steps.first() {
                        Some(GremlinStep::As(label)) if sub.alias_map.contains_key(label) => {
                            (sub.alias_map[label].clone(), &inner_steps[1..])
                        }
                        _ => (ctx.current_alias.clone(), inner_steps),
                    };

                    // Endpoint constraint: if the sub-traversal ends with
                    // as("label") referencing a bound match variable, the
                    // result must equal that variable's value.
                    let endpoint_target = if let Some(GremlinStep::As(label)) = compile_steps.last()
                    {
                        // Look up the label in the match label_map (use
                        // the alias that was bound by the join patterns).
                        label_map
                            .iter()
                            .find(|(l, _)| l == label)
                            .map(|(_, a)| a.clone())
                    } else {
                        None
                    };

                    let inner_op = sub.compile_seeded(compile_steps, &seed_alias)?;
                    self.absorb(&sub);
                    let sub_result_alias = sub.last_alias.clone();

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

                    let cnt_alias = self.next_alias("mcnt");
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
                        outer: Box::new(match_child),
                        inner: Box::new(count_op),
                    };

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
                }
            }

            GremlinStep::Optional(opt_steps) => {
                // optional(traversal): coalesce(traversal, identity)
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let mut sub = self.sub_compiler();
                let opt_op = sub.compile_seeded(opt_steps, &ctx.current_alias)?;
                self.absorb(&sub);
                // Merge sub-compiler's path tracking into local ctx.path_aliases
                // so path()/simplePath() see intermediate aliases from the
                // optional body (e.g., out() inside optional).
                for alias in &sub.path_tracking {
                    if !ctx.path_aliases.contains(alias) {
                        ctx.path_aliases.push(alias.clone());
                    }
                }
                ctx.current_alias = sub.last_alias.clone();
                ctx.current = Some(Operator::Coalesce {
                    alternatives: vec![opt_op, Operator::SingleRow],
                    child: Box::new(child),
                });
                if !ctx.path_aliases.contains(&ctx.current_alias) {
                    ctx.path_aliases.push(ctx.current_alias.clone());
                }
                if !self.path_tracking.contains(&ctx.current_alias) {
                    self.path_tracking.push(ctx.current_alias.clone());
                }
            }

            GremlinStep::Option(token, branch_steps) => {
                // Handle Merge.* tokens for mergeV/mergeE options
                let is_merge_option = matches!(
                    token,
                    GremlinValue::String(s) if s.starts_with("Merge.")
                );
                if is_merge_option {
                    // mergeV/mergeE options — no-op for now
                } else if let Some(Operator::ChooseMap {
                    ref mut options, ..
                }) = ctx.current
                {
                    let mut sub = self.sub_compiler();
                    let branch_op = sub.compile_seeded(branch_steps, &ctx.current_alias)?;
                    let branch_alias = sub.last_alias.clone();
                    self.absorb(&sub);
                    let match_val = match token {
                        GremlinValue::String(s) if s == "none" => None,
                        _ => Some(gremlin_value_to_value(token)),
                    };
                    options.push((match_val, Box::new(branch_op), Some(branch_alias)));
                } else if ctx.current.is_none() {
                    return Err(CompileError::MissingChild);
                }
                // If ctx.current is not ChooseMap or Merge, option is ignored (no-op).
            }

            // ── String steps ─────────────────────────────────────────────────
            _ => unreachable!(
                "compile_branch_step received non-branch variant: {:?}",
                step
            ),
        }
        #[allow(unreachable_code)]
        Ok(StepFlow::Proceed)
    }
}
