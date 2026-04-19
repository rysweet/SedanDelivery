//! `by` step variant: by() after dedup() — dedup by specific property.
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
    pub(super) fn handle_by_dedup(
        &mut self,
        key: &str,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        if ctx.dedup_by_consumed {
            return Err(CompileError::InvalidSequence(
                "Dedup step can only have one by modulator".into(),
            ));
        }
        // by() after dedup() — dedup by specific property
        if !key.is_empty() {
            if let Some(Operator::Distinct { ref mut by_key, .. }) = ctx.current {
                if let Some(ref existing) = *by_key {
                    if existing.contains('\x1f') {
                        // Multi-key dedup: apply property to each label
                        let new_key = existing
                            .split('\x1f')
                            .map(|l| format!("__dedup_prop__{}__{}", key, l))
                            .collect::<Vec<_>>()
                            .join("\x1f");
                        *by_key = Some(new_key);
                    } else {
                        *by_key = Some(format!("__dedup_prop__{}__{}", key, existing));
                    }
                } else {
                    *by_key = Some(format!("__dedup_prop__{}__{}", key, ctx.current_alias.0));
                }
            }
        }
        ctx.dedup_by_consumed = true;
        Ok(())
    }
}
