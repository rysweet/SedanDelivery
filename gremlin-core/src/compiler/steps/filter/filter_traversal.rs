//! `filter(traversal)` arm.
//!
//! Pure mechanical extraction from the original `filter.rs` — no behavior changes.

#![allow(unused_imports)]

use cypher_core::ast::{BinaryOp, Expr};
use cypher_core::planner::Operator;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::helpers::*;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    pub(super) fn compile_filter_traversal(
        &mut self,
        filter_steps: &[GremlinStep],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
        Ok(())
    }
}
