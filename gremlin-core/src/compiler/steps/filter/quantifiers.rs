//! `all`, `any`, and `none` collection-quantifier arms.
//!
//! Pure mechanical extraction from the original `filter.rs` — no behavior changes.

#![allow(unused_imports)]

use cypher_core::ast::Expr;
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::helpers::*;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::Predicate;

impl GremlinCompiler {
    pub(super) fn compile_all(
        &mut self,
        pred: &Predicate,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let (pred_op_str, pred_val_expr) = predicate_to_args(pred);
        let pred_fn = Expr::FunctionCall {
            name: SmolStr::new("gremlin_all"),
            args: vec![
                Expr::Variable(ctx.current_alias.clone()),
                Expr::Str(pred_op_str),
                pred_val_expr,
            ],
            distinct: false,
        };
        ctx.current = Some(Operator::Filter {
            predicate: pred_fn,
            child: Box::new(child),
        });
        Ok(())
    }

    pub(super) fn compile_any(
        &mut self,
        pred: &Predicate,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let (pred_op_str, pred_val_expr) = predicate_to_args(pred);
        let pred_fn = Expr::FunctionCall {
            name: SmolStr::new("gremlin_any"),
            args: vec![
                Expr::Variable(ctx.current_alias.clone()),
                Expr::Str(pred_op_str),
                pred_val_expr,
            ],
            distinct: false,
        };
        ctx.current = Some(Operator::Filter {
            predicate: pred_fn,
            child: Box::new(child),
        });
        Ok(())
    }

    pub(super) fn compile_none_pred(
        &mut self,
        pred: &Predicate,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let (pred_op_str, pred_val_expr) = predicate_to_args(pred);
        let pred_fn = Expr::FunctionCall {
            name: SmolStr::new("gremlin_none"),
            args: vec![
                Expr::Variable(ctx.current_alias.clone()),
                Expr::Str(pred_op_str),
                pred_val_expr,
            ],
            distinct: false,
        };
        ctx.current = Some(Operator::Filter {
            predicate: pred_fn,
            child: Box::new(child),
        });
        Ok(())
    }
}
