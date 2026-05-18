//! `by` step variant: by() after group()/groupCount() — modifies GremlinGroup.
//!
//! Pure mechanical extraction from the original `by.rs` — no behavior changes.

#![allow(unused_imports)]

use std::collections::HashMap;

use cypher_core::ast::{BinaryOp, Direction, Expr, Identifier, Label, RelType};
use cypher_core::planner::{
    AggFunction, CreateEdgeSpec, CreatePattern, GremlinPop, Operator, SetItem,
};
use smol_str::SmolStr;

use crate::bytecode::{GremlinStep, Predicate};
use crate::compiler::compile_from_context::CompileFromContext;
use crate::compiler::helpers::*;
use crate::compiler::{CompileError, GremlinCompiler, SelectByModulator};
use crate::types::GremlinValue;

impl GremlinCompiler {
    pub(super) fn handle_by_group(
        &mut self,
        key: &str,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // by() after group()/groupCount() — modifies GremlinGroup.
        // Use ctx.pre_group_alias (the alias before group() changed it).
        if ctx.pending_group_by_count == 0 {
            // First by() = group key
            if !key.is_empty() {
                // A non-empty key means the group key is no longer the identity.
                if let Some(Operator::GremlinGroup {
                    ref mut key_is_identity,
                    ..
                }) = ctx.current
                {
                    *key_is_identity = false;
                }
                // Try parsing __.prefix strings as sub-traversals
                if key.starts_with("__.") && key.contains('(') {
                    let no_param = |_: &str| -> Option<GremlinValue> { None };
                    let sub_steps = crate::text_parser::parse_inner_traversal(key, &no_param)
                        .map_err(|e| CompileError::InvalidSequence(e.to_string()))?;
                    if !sub_steps.is_empty() {
                        let mut sub = self.sub_compiler();
                        let sub_op = sub.compile_seeded(&sub_steps, &ctx.pre_group_alias)?;
                        let sub_out_alias = sub.last_alias.clone();
                        self.absorb(&sub);
                        if let Some(Operator::GremlinGroup {
                            ref mut key_traversal,
                            ..
                        }) = ctx.current
                        {
                            *key_traversal = Some((Box::new(sub_op), sub_out_alias));
                        }
                    }
                } else if let Some(Operator::GremlinGroup {
                    ref mut key_expr, ..
                }) = ctx.current
                {
                    *key_expr = make_by_expr(key, &ctx.pre_group_alias);
                }
            }
            ctx.pending_group_by_count = 1;
        } else if ctx.pending_group_is_count {
            // groupCount() only supports one by() modulator
            return Err(CompileError::InvalidSequence(
                "GroupCount step can only have one by modulator".into(),
            ));
        } else {
            // Second by() = value aggregation
            if key == "count()" || key == "__.count()" {
                if let Some(Operator::GremlinGroup { ref mut agg_fn, .. }) = ctx.current {
                    *agg_fn = AggFunction::Count;
                }
            } else if key == "fold()" || key == "__.fold()" {
                if let Some(Operator::GremlinGroup {
                    ref mut agg_fn,
                    ref mut explicit_fold,
                    ..
                }) = ctx.current
                {
                    *agg_fn = AggFunction::Collect;
                    *explicit_fold = true;
                }
            } else if key == "sum()" || key == "__.sum()" {
                if let Some(Operator::GremlinGroup { ref mut agg_fn, .. }) = ctx.current {
                    *agg_fn = AggFunction::Sum;
                }
            } else if key.is_empty() {
                // by() with no args — identity (collect the element)
                if let Some(Operator::GremlinGroup { ref mut agg_fn, .. }) = ctx.current {
                    *agg_fn = AggFunction::Collect;
                }
            } else if key.starts_with("__.") && key.contains('(') {
                // __.prefix value-by: parse as sub-traversal
                let no_param = |_: &str| -> Option<GremlinValue> { None };
                let sub_steps = crate::text_parser::parse_inner_traversal(key, &no_param)
                    .map_err(|e| CompileError::InvalidSequence(e.to_string()))?;
                if !sub_steps.is_empty() {
                    let (proj_steps, trailing_agg) = split_trailing_group_value_agg(&sub_steps);
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
                }
            } else if let Some(Operator::GremlinGroup {
                ref mut value_expr, ..
            }) = ctx.current
            {
                // by("propName") — collect property values
                *value_expr = make_by_expr(key, &ctx.pre_group_alias);
            }
            ctx.pending_group = false;
        }
        Ok(())
    }
}
