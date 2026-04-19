//! `by` step variant: by() after select() — normalize traversal-form modulators into compact keys.
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
    pub(super) fn handle_by_select(&mut self, key: &str, ctx: &mut CompileFromContext) {
        // by() after select() — normalize traversal-form
        // modulators like by(__.math("_+_")) into compact keys.
        if key.starts_with("__.") && key.contains('(') {
            let no_param = |_: &str| -> Option<GremlinValue> { None };
            if let Ok(sub_steps) = crate::text_parser::parse_inner_traversal(key, &no_param) {
                ctx.select_by_mods
                    .push(SelectByModulator::Key(extract_by_traversal_key(&sub_steps)));
            } else {
                ctx.select_by_mods
                    .push(SelectByModulator::Key(key.to_string()));
            }
        } else {
            ctx.select_by_mods
                .push(SelectByModulator::Key(key.to_string()));
        }
    }
}
