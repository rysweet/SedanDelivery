//! `by` category step compilation arms extracted from `dispatch.rs`.
//!
//! The original `by.rs` implementation was a single 776-LOC `compile_by_step`
//! function with one giant `if/else if` chain dispatching by which `pending_*`
//! flag was set on the [`CompileFromContext`]. This module preserves the
//! public API (`compile_by_step` on [`GremlinCompiler`]) while delegating
//! each variant to a dedicated submodule. The split is mechanical — no
//! behavior changes.

#![allow(unused_imports)]

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::Operator;

use super::super::compile_from_context::CompileFromContext;
use super::super::helpers::*;
use super::super::{CompileError, GremlinCompiler};
use super::StepFlow;
use crate::bytecode::GremlinStep;

mod aggregate;
mod dedup;
mod format;
mod group;
mod math;
mod order;
mod order_local;
mod project;
mod sack_op;
mod secondary_sort;
mod select;

impl GremlinCompiler {
    pub(super) fn compile_by_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::By(key, descending) => {
                let descending = *descending;

                if ctx.pending_math {
                    self.handle_by_math(key, ctx)?;
                } else if ctx.pending_order && ctx.pending_order_local {
                    self.handle_by_order_local(key, descending, ctx)?;
                } else if ctx.pending_order {
                    self.handle_by_order(key, descending, ctx)?;
                } else if !ctx.pending_order && matches!(ctx.current, Some(Operator::Sort { .. })) {
                    self.handle_by_secondary_sort(key, descending, ctx);
                } else if ctx.pending_group {
                    self.handle_by_group(key, ctx)?;
                } else if ctx.pending_select {
                    self.handle_by_select(key, ctx);
                } else if ctx.pending_path {
                    // by() after path() — collect by-key for path element transformation
                    ctx.path_by_keys.push(key.to_string());
                } else if ctx.pending_dedup {
                    self.handle_by_dedup(key, ctx)?;
                } else if ctx.pending_aggregate {
                    self.handle_by_aggregate(key, ctx);
                } else if ctx.pending_format {
                    self.handle_by_format(key, ctx)?;
                } else if ctx.pending_sack_op {
                    self.handle_by_sack_op(key, ctx);
                } else if ctx.pending_project {
                    self.handle_by_project(key, ctx)?;
                } else if !ctx.pending_where_by_targets.is_empty() {
                    // by() after where() records a traversal-ring modulator
                    // and applies it only once the full by-list is known.
                    ctx.pending_where_by_modulators.push(key.to_string());
                } else if ctx.aggregate_by_consumed {
                    return Err(CompileError::InvalidSequence(
                        "Aggregate step can only have one by modulator".into(),
                    ));
                } else if ctx.pending_sample && ctx.sample_by_consumed {
                    return Err(CompileError::InvalidSequence(
                        "Sample step can only have one by modulator".into(),
                    ));
                } else if ctx.pending_sack_by_consumed {
                    return Err(CompileError::InvalidSequence(
                        "Sack step can only have one by modulator".into(),
                    ));
                } else if ctx.pending_valuemap && ctx.valuemap_by_consumed {
                    return Err(CompileError::InvalidSequence(
                        "step can only have one by modulator".into(),
                    ));
                }

                // Track by() consumption for sample/sack/valueMap
                if ctx.pending_sample && !ctx.sample_by_consumed {
                    ctx.sample_by_consumed = true;
                    ctx.pending_sample_weight = Some(if key.is_empty() {
                        Expr::Variable(ctx.current_alias.clone())
                    } else {
                        make_by_expr(key, &ctx.current_alias)
                    });
                }
                if ctx.pending_sack_op && !ctx.pending_sack_by_consumed {
                    ctx.pending_sack_by_consumed = true;
                }
                if ctx.pending_valuemap && !ctx.valuemap_by_consumed {
                    ctx.valuemap_by_consumed = true;
                }
            }

            _ => unreachable!("compile_by_step received non-by variant: {:?}", step),
        }
        Ok(StepFlow::Proceed)
    }
}
