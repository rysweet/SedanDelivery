//! `by` step variant: Additional by() after a Sort — add secondary sort key.
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
    pub(super) fn handle_by_secondary_sort(
        &mut self,
        key: &str,
        descending: bool,
        ctx: &mut CompileFromContext,
    ) {
        // Additional by() after a Sort — add secondary sort key
        let sort_expr = if key.is_empty() {
            Expr::Variable(ctx.current_alias.clone())
        } else {
            let prop = Expr::Property(
                Box::new(Expr::Variable(ctx.current_alias.clone())),
                SmolStr::new(key),
            );
            if self.productive_by {
                Expr::FunctionCall {
                    name: SmolStr::new("coalesce"),
                    args: vec![prop, Expr::Null],
                    distinct: false,
                }
            } else {
                prop
            }
        };
        if let Some(Operator::Sort { ref mut items, .. }) = ctx.current {
            items.push((sort_expr, !descending));
        }
    }
}
