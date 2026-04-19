//! `where` and `not` arms.
//!
//! Pure mechanical extraction from the original `filter.rs` — no behavior changes.

#![allow(unused_imports)]

use cypher_core::ast::{BinaryOp, Expr};
use cypher_core::planner::{AggFunction, Operator};
use smol_str::SmolStr;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::helpers::*;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    pub(super) fn compile_where_or_not(
        &mut self,
        inner_steps: &[GremlinStep],
        is_not: bool,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;

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
                _ => (ctx.current_alias.clone(), inner_steps),
            };

            // Check if the sub-traversal ends with as("label")
            // referencing an already-bound variable.  If so, the
            // endpoint must be constrained to match that binding.
            let endpoint_target = if let Some(GremlinStep::As(label)) = steps_slice.last() {
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
                if let Some(targets) =
                    extract_where_by_targets(inner_steps, &self.alias_map, &ctx.current_alias)
                {
                    ctx.pending_where_by_targets = targets;
                    ctx.pending_where_by_modulators.clear();
                }
            }
            ctx.current = Some(Operator::Filter {
                predicate,
                child: Box::new(child),
            });
        }
        Ok(())
    }
}
