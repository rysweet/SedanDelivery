//! `mutation` category step compilation arms extracted from `dispatch.rs`.
//!
//! Pure mechanical extraction — no behavior changes. Each arm body is
//! verbatim from the corresponding case in the original monolithic match.
//!
//! This module is split by mutation-step variant into focused submodules.
//! `mod.rs` only contains the dispatcher; each submodule implements the
//! per-variant compilation logic as `impl GremlinCompiler` methods.

#![allow(unused_imports)]

use super::super::compile_from_context::CompileFromContext;
use super::super::{CompileError, GremlinCompiler};
use super::StepFlow;
use crate::bytecode::GremlinStep;

mod add;
mod drop;
mod from_to;
mod merge;
mod property;

impl GremlinCompiler {
    pub(super) fn compile_mutation_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::AddV(label) => self.compile_add_v(label, ctx)?,
            GremlinStep::AddVTraversal(label_steps) => {
                self.compile_add_v_traversal(label_steps, ctx)?
            }
            GremlinStep::AddE(label) => self.compile_add_e(label, ctx)?,

            GremlinStep::MergeV(search_map) => self.compile_merge_v(search_map, ctx)?,
            GremlinStep::MergeE(search_map) => self.compile_merge_e(search_map, ctx)?,

            GremlinStep::Property(key, value) => self.compile_property(key, value, ctx)?,

            GremlinStep::From(_) | GremlinStep::To(_) => {
                return self.compile_from_to(step, ctx);
            }

            GremlinStep::Drop => self.compile_drop(ctx)?,

            _ => unreachable!(
                "compile_mutation_step received non-mutation variant: {:?}",
                step
            ),
        }
        Ok(StepFlow::Proceed)
    }
}
