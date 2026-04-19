//! `project()` step compilation.

#![allow(unused_imports)]

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};

impl GremlinCompiler {
    pub(super) fn compile_project(
        &mut self,
        keys: &[String],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        ctx.pre_project_alias = ctx.current_alias.clone();
        let mut items: Vec<(Expr, Option<Identifier>)> = keys
            .iter()
            .map(|k| {
                (
                    Expr::Property(
                        Box::new(Expr::Variable(ctx.current_alias.clone())),
                        SmolStr::new(k),
                    ),
                    Some(Identifier::new(k.clone())),
                )
            })
            .collect();
        items.extend(self.label_carry_items());
        ctx.current = Some(Operator::Project {
            items,
            child: Box::new(child),
        });
        ctx.pending_project = true;
        ctx.project_keys = keys.to_vec();
        ctx.project_by_count = 0;
        Ok(())
    }
}
