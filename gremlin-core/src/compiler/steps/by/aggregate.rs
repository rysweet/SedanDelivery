//! `by` step variant: by() after aggregate() — project a property from each traverser.
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
    pub(super) fn handle_by_aggregate(&mut self, key: &str, ctx: &mut CompileFromContext) {
        // by() after aggregate() — project a property from each traverser
        if let Some(Operator::GremlinAggregate {
            ref mut by_keys, ..
        }) = ctx.current
        {
            if !key.is_empty() {
                by_keys.push(key.to_string());
            }
        }
        ctx.pending_aggregate = false;
        ctx.aggregate_by_consumed = true;
    }
}
