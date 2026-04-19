//! `by` step variant: by() after order(Scope.local) — sort within list by key.
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
    pub(super) fn handle_by_order_local(
        &mut self,
        key: &str,
        descending: bool,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // by() after order(Scope.local) — sort within list by key
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let alias = self.next_alias("srtl");
        let by_key = if key.is_empty() { "" } else { key };
        let desc_str = if descending { "desc" } else { "asc" };
        let mut items = vec![(
            Expr::FunctionCall {
                name: SmolStr::new("gremlin_sort_local"),
                args: vec![
                    Expr::Variable(ctx.current_alias.clone()),
                    Expr::Str(by_key.to_string().into_boxed_str()),
                    Expr::Str(desc_str.to_string().into_boxed_str()),
                ],
                distinct: false,
            },
            Some(alias.clone()),
        )];
        items.extend(self.label_carry_items());
        ctx.current = Some(Operator::Project {
            items,
            child: Box::new(child),
        });
        ctx.current_alias = alias;
        ctx.pending_order = false;
        ctx.pending_order_local = false;
        Ok(())
    }
}
