//! `simplePath`, `cyclicPath`, and `coin` arms.
//!
//! Pure mechanical extraction from the original `filter.rs` — no behavior changes.

#![allow(unused_imports)]

use cypher_core::ast::{BinaryOp, Expr};
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};

impl GremlinCompiler {
    pub(super) fn compile_simple_path(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        ctx.current = Some(Operator::GremlinSimplePath {
            child: Box::new(child),
            path_aliases: ctx.path_aliases.clone(),
            by_keys: vec![],
            from_label: None,
            to_label: None,
        });
        Ok(())
    }

    pub(super) fn compile_cyclic_path(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        ctx.current = Some(Operator::GremlinCyclicPath {
            child: Box::new(child),
            path_aliases: ctx.path_aliases.clone(),
            by_keys: vec![],
            from_label: None,
            to_label: None,
        });
        ctx.pending_path = true;
        ctx.path_by_keys.clear();
        Ok(())
    }

    pub(super) fn compile_coin(
        &mut self,
        probability: f64,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let rand_expr = if let Some(seed) = self.seed {
            // Seeded: use Java-compatible LCG for deterministic results
            Expr::FunctionCall {
                name: SmolStr::new("java_random_next_double"),
                args: vec![Expr::Integer(seed)],
                distinct: false,
            }
        } else {
            Expr::FunctionCall {
                name: SmolStr::new("rand"),
                args: vec![],
                distinct: false,
            }
        };
        ctx.current = Some(Operator::Filter {
            predicate: Expr::BinaryOp {
                op: BinaryOp::Lt,
                left: Box::new(rand_expr),
                right: Box::new(Expr::Float(probability)),
            },
            child: Box::new(child),
        });
        Ok(())
    }
}
