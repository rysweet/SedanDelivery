//! `id()`, `label()`, `identity()`, `element()`, and `index()` step compilation.

#![allow(unused_imports)]

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    pub(super) fn compile_id(&mut self, ctx: &mut CompileFromContext) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let id_alias = self.next_alias("id");
        let mut items = vec![(
            Expr::FunctionCall {
                name: SmolStr::new("id"),
                args: vec![Expr::Variable(ctx.current_alias.clone())],
                distinct: false,
            },
            Some(id_alias.clone()),
        )];
        items.extend(self.label_carry_items());
        ctx.current = Some(Operator::Project {
            items,
            child: Box::new(child),
        });
        ctx.current_alias = id_alias;
        Ok(())
    }

    pub(super) fn compile_label(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let lbl_alias = self.next_alias("lbl");
        let mut items = vec![(
            Expr::FunctionCall {
                name: SmolStr::new("gremlin_single_label"),
                args: vec![Expr::Variable(ctx.current_alias.clone())],
                distinct: false,
            },
            Some(lbl_alias.clone()),
        )];
        items.extend(self.label_carry_items());
        ctx.current = Some(Operator::Project {
            items,
            child: Box::new(child),
        });
        ctx.current_alias = lbl_alias;
        Ok(())
    }

    pub(super) fn compile_identity(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // No-op: identity() passes through the ctx.current traverser unchanged.
        if ctx.current.is_none() {
            return Err(CompileError::MissingChild);
        }
        Ok(())
    }

    pub(super) fn compile_element(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // element() — extract ~element from property Map objects.
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let elem_alias = self.next_alias("elem");
        let mut items = vec![(
            Expr::Property(
                Box::new(Expr::Variable(ctx.current_alias.clone())),
                SmolStr::new("~element"),
            ),
            Some(elem_alias.clone()),
        )];
        items.extend(self.label_carry_items());
        ctx.current = Some(Operator::Filter {
            predicate: Expr::IsNotNull(Box::new(Expr::Variable(elem_alias.clone()))),
            child: Box::new(Operator::Project {
                items,
                child: Box::new(child),
            }),
        });
        ctx.current_alias = elem_alias;
        Ok(())
    }

    pub(super) fn compile_index(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let out_alias = self.next_alias("idx");
        let with_map = matches!(step, GremlinStep::IndexWithMap);
        ctx.current = Some(Operator::GremlinIndex {
            child: Box::new(child),
            alias: ctx.current_alias.clone(),
            out_alias: out_alias.clone(),
            with_map,
        });
        ctx.current_alias = out_alias;
        Ok(())
    }
}
