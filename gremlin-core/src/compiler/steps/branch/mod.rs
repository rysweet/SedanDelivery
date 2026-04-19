//! `branch` category step compilation arms extracted from `dispatch.rs`.
//!
//! Pure mechanical extraction — no behavior changes. Each arm body is
//! verbatim from the corresponding case in the original monolithic match.
//!
//! This module is split by branch-step variant into focused submodules.
//! `mod.rs` only contains the dispatcher; each submodule implements the
//! per-variant compilation logic as `impl GremlinCompiler` methods.

#![allow(unused_imports)]

use cypher_core::planner::Operator;

use super::super::compile_from_context::CompileFromContext;
use super::super::{CompileError, GremlinCompiler};
use super::StepFlow;
use crate::bytecode::GremlinStep;

mod choose;
mod coalesce;
mod match_step;
mod repeat;
mod union;

impl GremlinCompiler {
    pub(super) fn compile_branch_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::Repeat(label, body_steps) => {
                self.compile_repeat(label, body_steps, ctx)?;
            }
            GremlinStep::Times(n) => self.compile_times(*n, ctx),
            GremlinStep::Until(until_steps) => self.compile_until(until_steps, ctx)?,
            GremlinStep::Emit => self.compile_emit(ctx),
            GremlinStep::EmitPred(pred_steps) => self.compile_emit_pred(pred_steps, ctx)?,

            GremlinStep::Choose(pred_steps, true_steps, false_steps) => {
                self.compile_choose(pred_steps, true_steps, false_steps.as_deref(), ctx)?;
            }
            GremlinStep::Option(token, branch_steps) => {
                self.compile_option(token, branch_steps, ctx)?;
            }

            GremlinStep::Coalesce(alternatives) => self.compile_coalesce(alternatives, ctx)?,
            GremlinStep::Optional(opt_steps) => self.compile_optional(opt_steps, ctx)?,

            GremlinStep::Union(branches) => self.compile_union(branches, ctx)?,

            GremlinStep::Match(patterns) => self.compile_match(patterns, ctx)?,

            _ => unreachable!(
                "compile_branch_step received non-branch variant: {:?}",
                step
            ),
        }
        Ok(StepFlow::Proceed)
    }
}
