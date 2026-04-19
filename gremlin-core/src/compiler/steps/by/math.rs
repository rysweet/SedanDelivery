//! `by` step variant: by() after math() — resolve variable references with property key.
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
    pub(super) fn handle_by_math(
        &mut self,
        key: &str,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // by() after math() — resolve variable references with property key
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let alias = self.next_alias("math");
        let mut args = vec![
            Expr::Variable(ctx.pre_math_alias.clone()),
            Expr::Str(ctx.pending_math_expr.clone().into_boxed_str()),
        ];
        // For each variable in the expression, resolve using the by key
        for var_name in extract_math_variables(&ctx.pending_math_expr) {
            if let Some(stacks) = self.label_stacks.get(&var_name) {
                if let Some(label_alias) = stacks.last() {
                    args.push(Expr::Str(var_name.into_boxed_str()));
                    if key.is_empty() {
                        args.push(Expr::Variable(label_alias.clone()));
                    } else {
                        args.push(Expr::Property(
                            Box::new(Expr::Variable(label_alias.clone())),
                            SmolStr::new(key),
                        ));
                    }
                }
            } else if let Some(se_ident) = self.alias_map.get(&var_name) {
                args.push(Expr::Str(var_name.into_boxed_str()));
                if key.is_empty() {
                    args.push(Expr::Variable(se_ident.clone()));
                } else {
                    args.push(Expr::Property(
                        Box::new(Expr::Variable(se_ident.clone())),
                        SmolStr::new(key),
                    ));
                }
            }
        }
        let mut items = vec![(
            Expr::FunctionCall {
                name: SmolStr::new("gremlin_math"),
                args,
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
        ctx.pending_math = false;
        ctx.pending_math_expr.clear();
        Ok(())
    }
}
