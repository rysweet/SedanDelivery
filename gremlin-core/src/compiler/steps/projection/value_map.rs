//! `valueMap()` and `elementMap()` step compilation.

#![allow(unused_imports)]

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};

impl GremlinCompiler {
    pub(super) fn compile_value_map(
        &mut self,
        keys: &[String],
        token_mode: u8,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let alias = self.next_alias("vmap");
        let mut args = vec![
            Expr::Variable(ctx.current_alias.clone()),
            Expr::Integer(i64::from(token_mode)),
        ];
        for k in keys {
            args.push(Expr::Str(k.as_str().into()));
        }
        let fc = Expr::FunctionCall {
            name: SmolStr::new("gremlin_valuemap"),
            args,
            distinct: false,
        };
        let mut items = vec![(fc, Some(alias.clone()))];
        items.extend(self.label_carry_items());
        ctx.current_alias = alias;
        ctx.current = Some(Operator::Project {
            items,
            child: Box::new(child),
        });
        ctx.pending_valuemap = true;
        ctx.valuemap_by_consumed = false;
        Ok(())
    }

    pub(super) fn compile_element_map(
        &mut self,
        keys: &[String],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let alias = self.next_alias("emap");
        // elementMap as a function call that returns a Map value
        let mut args = vec![Expr::Variable(ctx.current_alias.clone())];
        for k in keys {
            args.push(Expr::Str(k.as_str().into()));
        }
        let fc = Expr::FunctionCall {
            name: SmolStr::new("gremlin_elementmap"),
            args,
            distinct: false,
        };
        let mut items = vec![(fc, Some(alias.clone()))];
        items.extend(self.label_carry_items());
        ctx.current_alias = alias;
        ctx.current = Some(Operator::Project {
            items,
            child: Box::new(child),
        });
        Ok(())
    }
}
