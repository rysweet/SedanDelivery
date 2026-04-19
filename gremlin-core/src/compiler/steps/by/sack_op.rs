//! `by` step variant: by(key) after sack(op) — rewrite the sack Project to use a property/label expression.
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
    pub(super) fn handle_by_sack_op(&mut self, key: &str, ctx: &mut CompileFromContext) {
        // by(key) after sack(op) — rewrite the sack Project to
        // use a property/label expression instead of ctx.current_alias.
        if !key.is_empty() {
            let by_expr = make_by_expr(key, &ctx.current_alias);
            if let Some(Operator::Project { ref mut items, .. }) = ctx.current {
                // First item is the sack assignment; rewrite the rhs
                // (ctx.current_alias reference) with the by expression.
                if let Some((expr, _)) = items.first_mut() {
                    rewrite_sack_by(expr, &ctx.current_alias, &by_expr);
                }
            }
            // Filter out traversers where the by-key property is null
            // (Gremlin drops traversers when by() can't resolve a value).
            let sack_var = Identifier::new("__sack");
            if let Some(proj) = ctx.current.take() {
                ctx.current = Some(Operator::Filter {
                    predicate: Expr::IsNotNull(Box::new(Expr::Variable(sack_var))),
                    child: Box::new(proj),
                });
            }
        }
        ctx.pending_sack_op = false;
        ctx.pending_sack_by_consumed = true;
    }
}
