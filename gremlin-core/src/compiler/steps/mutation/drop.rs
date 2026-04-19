//! `drop()` step compilation.

#![allow(unused_imports)]

use cypher_core::ast::Expr;
use cypher_core::planner::Operator;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};

impl GremlinCompiler {
    pub(super) fn compile_drop(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        if self.read_only {
            return Err(CompileError::ReadOnlyViolation);
        }
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        ctx.current = Some(Operator::Delete {
            exprs: vec![Expr::Variable(ctx.current_alias.clone())],
            detach: true,
            child: Box::new(child),
        });
        Ok(())
    }
}
