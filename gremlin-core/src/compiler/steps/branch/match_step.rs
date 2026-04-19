//! `match()` pattern compilation including post-filter `not`/`where` patterns.

#![allow(unused_imports)]

use cypher_core::ast::{BinaryOp, Expr, Identifier};
use cypher_core::planner::Operator;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    pub(super) fn compile_match(
        &mut self,
        patterns: &[Vec<GremlinStep>],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
        fn flatten_match_patterns(patterns: &[Vec<GremlinStep>], out: &mut Vec<Vec<GremlinStep>>) {
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
            let endpoint_target = if let Some(GremlinStep::As(label)) = compile_steps.last() {
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
        Ok(())
    }
}
