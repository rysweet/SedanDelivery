//! `projection` category step compilation arms extracted from `dispatch.rs`.
//!
//! Pure mechanical extraction — no behavior changes. Each arm body is
//! verbatim from the corresponding case in the original monolithic match.
//!
//! This module is split by projection-step variant into focused submodules.
//! `mod.rs` only contains the dispatcher; each submodule implements the
//! per-variant compilation logic as `impl GremlinCompiler` methods.

#![allow(unused_imports)]

use super::super::compile_from_context::CompileFromContext;
use super::super::{CompileError, GremlinCompiler};
use super::StepFlow;
use crate::bytecode::GremlinStep;

mod map;
mod meta;
mod project;
mod properties;
mod value_map;
mod values;

impl GremlinCompiler {
    pub(super) fn compile_projection_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::Values(keys) => self.compile_values(keys, ctx)?,

            GremlinStep::ValueMap(keys, token_mode) => {
                self.compile_value_map(keys, *token_mode, ctx)?
            }

            GremlinStep::Properties(keys) => self.compile_properties(keys, ctx)?,

            GremlinStep::Id => self.compile_id(ctx)?,

            GremlinStep::Label => self.compile_label(ctx)?,

            GremlinStep::Project(keys) => self.compile_project(keys, ctx)?,

            GremlinStep::Identity => self.compile_identity(ctx)?,

            GremlinStep::Element => self.compile_element(ctx)?,

            GremlinStep::Index | GremlinStep::IndexWithMap => self.compile_index(step, ctx)?,

            GremlinStep::ElementMap(keys) => self.compile_element_map(keys, ctx)?,

            GremlinStep::Map(map_steps) => self.compile_map(map_steps, ctx)?,

            GremlinStep::FlatMap(fm_steps) => self.compile_flat_map(fm_steps, ctx)?,

            GremlinStep::Tree(key) => self.compile_tree(key, ctx)?,

            _ => unreachable!(
                "compile_projection_step received non-projection variant: {:?}",
                step
            ),
        }
        Ok(StepFlow::Proceed)
    }
}
