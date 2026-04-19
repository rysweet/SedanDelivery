//! `side_effect` category step compilation arms extracted from `dispatch.rs`.
//!
//! Pure mechanical extraction — no behavior changes. Each arm body is
//! verbatim from the corresponding case in the original monolithic match.
//!
//! This module is split by side-effect-step variant into focused submodules.
//! `mod.rs` only contains the dispatcher; each submodule implements the
//! per-variant compilation logic as `impl GremlinCompiler` methods.

#![allow(unused_imports)]

use super::super::compile_from_context::CompileFromContext;
use super::super::{CompileError, GremlinCompiler};
use super::StepFlow;
use crate::bytecode::GremlinStep;

mod aggregate;
mod as_select;
mod barrier;
mod sack;

impl GremlinCompiler {
    pub(super) fn compile_side_effect_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::As(alias) => self.compile_as(alias, ctx),

            GremlinStep::Select(pop, keys) => self.compile_select(*pop, keys, ctx)?,

            GremlinStep::SideEffect(effect_steps) => {
                self.compile_side_effect_branch(effect_steps, ctx)?
            }

            GremlinStep::Aggregate(key) => self.compile_aggregate(key, ctx)?,

            GremlinStep::Cap(keys) => self.compile_cap(keys, ctx)?,

            GremlinStep::None => self.compile_none(ctx)?,

            GremlinStep::Barrier => self.compile_barrier(ctx)?,

            GremlinStep::BarrierNormSack => self.compile_barrier_norm_sack(ctx)?,

            GremlinStep::Discard => self.compile_discard(ctx)?,

            GremlinStep::Loops(ref label) => self.compile_loops(label, ctx),

            GremlinStep::Sack => self.compile_sack(ctx),

            GremlinStep::SackOp(op) => return self.compile_sack_op(op, ctx),

            GremlinStep::WithSack(val, ref merge_op) => self.compile_with_sack(val, merge_op, ctx),

            GremlinStep::WithBulkFalse => {
                ctx.with_bulk_false = true;
            }

            GremlinStep::WithSideEffect(key, val, ref reduce_op) => {
                self.compile_with_side_effect(key, val, reduce_op, ctx)
            }

            _ => unreachable!(
                "compile_side_effect_step received non-side_effect variant: {:?}",
                step
            ),
        }
        Ok(StepFlow::Proceed)
    }
}
