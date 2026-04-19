//! `order` category step compilation arms extracted from `dispatch.rs`.
//!
//! Pure mechanical extraction — no behavior changes. Each arm body is
//! verbatim from the corresponding case in the original monolithic match.
//!
//! This module is split by responsibility:
//! - `mod.rs`        — dispatcher for `Order` and `ByTraversal` steps
//! - `by_valuemap`   — `by(traversal)` after `valueMap()` (unfold lists)
//! - `by_project`    — `by(traversal)` after `project()`
//! - `by_group`      — `by(traversal)` after `group()` (key / value)
//! - `by_misc`       — `by(traversal)` after order/dedup/aggregate/format/sack/etc.

use super::super::compile_from_context::CompileFromContext;
use super::super::{CompileError, GremlinCompiler};
use super::StepFlow;
use crate::bytecode::GremlinStep;

mod by_group;
mod by_misc;
mod by_project;
mod by_valuemap;

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

            GremlinStep::ByTraversal(sub_steps, descending) => {
                // by(traversal) after valueMap — unfold list values
                if ctx.pending_valuemap && !ctx.valuemap_by_consumed {
                    self.apply_valuemap_unfold_by(sub_steps, ctx)?;
                    ctx.valuemap_by_consumed = true;
                } else if ctx.pending_valuemap && ctx.valuemap_by_consumed {
                    return Err(CompileError::InvalidSequence(
                        "step can only have one by modulator".into(),
                    ));
                } else if ctx.pending_project {
                    // by(traversal) after project() — compile the sub-traversal
                    // and convert to GremlinProject if not already done.
                    if ctx.project_by_count < ctx.project_keys.len() {
                        self.apply_project_by_traversal(sub_steps, ctx)?;
                    }
                }
                // Non-project contexts: delegate ByTraversal to the same
                // behavior as By by reconstructing the original key string.
                if !ctx.pending_project {
                    self.apply_non_project_by_traversal(sub_steps, *descending, ctx)?;
                }
            }

            _ => unreachable!("compile_order_step received non-order variant: {:?}", step),
        }
        Ok(StepFlow::Proceed)
    }

    fn apply_non_project_by_traversal(
        &mut self,
        sub_steps: &[GremlinStep],
        descending: bool,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        if ctx.pending_order {
            self.apply_order_by_traversal(sub_steps, descending, ctx)?;
        } else if ctx.pending_select {
            // by(traversal) after select() — preserve the
            // real traversal for per-key execution.
            ctx.select_by_mods
                .push(super::super::SelectByModulator::Traversal(
                    sub_steps.to_vec(),
                ));
        } else if ctx.pending_path {
            // by(traversal) after path() — extract property key
            // from patterns like by(values('name').toUpper())
            let key = super::super::helpers::extract_by_traversal_key(sub_steps);
            ctx.path_by_keys.push(key);
        } else if ctx.pending_dedup {
            self.apply_dedup_by_traversal(sub_steps, ctx)?;
        } else if !ctx.pending_where_by_targets.is_empty() {
            ctx.pending_where_by_modulators
                .push(super::super::helpers::extract_by_traversal_key(sub_steps));
        } else if ctx.pending_group {
            self.apply_group_by_traversal(sub_steps, ctx)?;
        } else if ctx.pending_aggregate {
            self.apply_aggregate_by_traversal(sub_steps, ctx)?;
        } else if ctx.pending_format {
            self.apply_format_by_traversal(sub_steps, ctx)?;
        } else if ctx.pending_sack_op && !ctx.pending_sack_by_consumed {
            self.apply_sack_by_traversal(sub_steps, ctx)?;
        }
        Ok(())
    }
}
