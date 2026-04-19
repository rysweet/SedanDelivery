//! `and`, `or`, and `is` arms.
//!
//! Pure mechanical extraction from the original `filter.rs` — no behavior changes.

#![allow(unused_imports)]

use cypher_core::ast::{BinaryOp, Expr};
use cypher_core::planner::Operator;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::helpers::*;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::{GremlinStep, Predicate};
use crate::types::GremlinValue;

impl GremlinCompiler {
    pub(super) fn compile_and(
        &mut self,
        branches: &[Vec<GremlinStep>],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        if branches.is_empty() {
            ctx.current = Some(child);
        } else {
            ctx.current = Some(self.compile_connective_branches(
                branches,
                &ctx.current_alias,
                child,
                BinaryOp::And,
            )?);
        }
        Ok(())
    }

    pub(super) fn compile_or(
        &mut self,
        branches: &[Vec<GremlinStep>],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        if branches.is_empty() {
            ctx.current = Some(child);
        } else {
            ctx.current = Some(self.compile_connective_branches(
                branches,
                &ctx.current_alias,
                child,
                BinaryOp::Or,
            )?);
        }
        Ok(())
    }

    pub(super) fn compile_is(
        &mut self,
        predicate: &Predicate,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let pred_expr = if let Predicate::IsType(GremlinValue::String(expected)) = predicate {
            if let Some(ref subtype) = self.numeric_subtype {
                if subtype == expected {
                    Expr::Bool(true)
                } else {
                    Expr::Bool(false)
                }
            } else {
                predicate_to_expr(predicate, &ctx.current_alias)
            }
        } else if let Some(label_expr) =
            predicate_label_ref(predicate, &self.alias_map, &self.aggregate_keys)
        {
            let op = predicate_to_binary_op(predicate);
            Expr::BinaryOp {
                op,
                left: Box::new(Expr::Variable(ctx.current_alias.clone())),
                right: Box::new(label_expr),
            }
        } else if predicate_has_label_refs(predicate, &self.alias_map, &self.aggregate_keys) {
            predicate_to_label_aware_expr(
                predicate,
                &Expr::Variable(ctx.current_alias.clone()),
                &self.alias_map,
                &self.aggregate_keys,
            )
        } else {
            predicate_to_expr(predicate, &ctx.current_alias)
        };
        ctx.current = Some(Operator::Filter {
            predicate: pred_expr,
            child: Box::new(child),
        });
        Ok(())
    }
}
