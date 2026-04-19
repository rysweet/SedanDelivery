//! `by` step variant: by(key) after format() — compile by-key to a property access.
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
    pub(super) fn handle_by_format(
        &mut self,
        key: &str,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // by(key) after format() — compile by-key to a property access
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let by_alias = self.next_alias("fby");
        let by_expr = if key.is_empty() {
            Expr::Variable(ctx.format_pre_alias.clone())
        } else {
            make_by_expr(key, &ctx.format_pre_alias)
        };
        let mut items = vec![(by_expr, Some(by_alias.clone()))];
        items.extend(self.label_carry_items());
        ctx.current = Some(Operator::Project {
            items,
            child: Box::new(child),
        });
        ctx.format_by_aliases.push(by_alias);
        Ok(())
    }
}
