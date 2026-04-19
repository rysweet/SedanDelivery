//! `by(traversal)` modulator after `group()` — first By becomes the group
//! key, second By becomes the value aggregation (with optional trailing
//! count/sum/fold).

use cypher_core::planner::{AggFunction, Operator};

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::helpers::split_trailing_group_value_agg;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    pub(super) fn apply_group_by_traversal(
        &mut self,
        sub_steps: &[GremlinStep],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        if ctx.pending_group_by_count == 0 {
            // First by(traversal) = group key — traversal is not identity
            if let Some(Operator::GremlinGroup {
                ref mut key_is_identity,
                ..
            }) = ctx.current
            {
                *key_is_identity = false;
            }
            let mut sub = self.sub_compiler();
            let sub_op = sub.compile_seeded(sub_steps, &ctx.pre_group_alias)?;
            let sub_out_alias = sub.last_alias.clone();
            self.absorb(&sub);
            if let Some(Operator::GremlinGroup {
                ref mut key_traversal,
                ..
            }) = ctx.current
            {
                *key_traversal = Some((Box::new(sub_op), sub_out_alias));
            }
            ctx.pending_group_by_count = 1;
        } else {
            // Second by(traversal) = value aggregation
            // Check for trailing agg step (count/sum/fold)
            let (proj_steps, trailing_agg) = split_trailing_group_value_agg(sub_steps);

            if proj_steps.is_empty() {
                if let Some(Operator::GremlinGroup {
                    ref mut agg_fn,
                    ref mut explicit_fold,
                    ..
                }) = ctx.current
                {
                    if let Some(agg) = trailing_agg {
                        if matches!(agg, AggFunction::Collect) {
                            *explicit_fold = true;
                        }
                        *agg_fn = agg;
                    }
                }
            } else {
                let mut sub = self.sub_compiler();
                // If the element going into group() is a single-entry Map
                // from unfold() (i.e., a Map.Entry), mark the sub-compiler
                // so that select(Column.keys/values) uses map-entry semantics
                // (returning the key/value directly, not a collection).
                let outer_child_is_unwind =
                    if let Some(Operator::GremlinGroup { child, .. }) = &ctx.current {
                        matches!(child.as_ref(), Operator::Unwind { .. })
                    } else {
                        false
                    };
                sub.seed_is_map_entry = outer_child_is_unwind;
                let sub_op = sub.compile_seeded(proj_steps, &ctx.pre_group_alias)?;
                let sub_out_alias = sub.last_alias.clone();
                self.absorb(&sub);
                if let Some(Operator::GremlinGroup {
                    ref mut value_traversal,
                    ref mut agg_fn,
                    ref mut explicit_fold,
                    ..
                }) = ctx.current
                {
                    *value_traversal = Some((Box::new(sub_op), sub_out_alias));
                    if let Some(agg) = trailing_agg {
                        if matches!(agg, AggFunction::Collect) {
                            *explicit_fold = true;
                        }
                        *agg_fn = agg;
                    }
                }
            }
            ctx.pending_group = false;
        }
        Ok(())
    }
}
