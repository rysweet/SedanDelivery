//! In-place / projection-shaped string mutators:
//! `Replace`, `Split`, `Substring`, `SubstringLocal`.

use cypher_core::ast::Expr;
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};

impl GremlinCompiler {
    pub(super) fn compile_str_replace(
        &mut self,
        old: &str,
        new: &str,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let alias = self.next_alias("str");
        let mut items = vec![(
            Expr::FunctionCall {
                name: SmolStr::new("gremlin_replace"),
                args: vec![
                    Expr::Variable(ctx.current_alias.clone()),
                    Expr::Str(old.to_string().into_boxed_str()),
                    Expr::Str(new.to_string().into_boxed_str()),
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
        Ok(())
    }

    pub(super) fn compile_str_split(
        &mut self,
        sep: &str,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let alias = self.next_alias("split");
        let mut items = vec![(
            Expr::FunctionCall {
                name: SmolStr::new("gremlin_split"),
                args: vec![
                    Expr::Variable(ctx.current_alias.clone()),
                    Expr::Str(sep.to_string().into_boxed_str()),
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
        Ok(())
    }

    pub(super) fn compile_str_substring(
        &mut self,
        start: i64,
        len: Option<i64>,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        self.emit_substring(ctx, "gremlin_substring", "substr", start, len)
    }

    pub(super) fn compile_str_substring_local(
        &mut self,
        start: i64,
        len: Option<i64>,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        self.emit_substring(ctx, "gremlin_substring_local", "substrl", start, len)
    }

    fn emit_substring(
        &mut self,
        ctx: &mut CompileFromContext,
        fn_name: &str,
        alias_seed: &str,
        start: i64,
        len: Option<i64>,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let alias = self.next_alias(alias_seed);
        let mut args = vec![
            Expr::Variable(ctx.current_alias.clone()),
            Expr::Integer(start),
        ];
        if let Some(l) = len {
            args.push(Expr::Integer(l));
        }
        let mut items = vec![(
            Expr::FunctionCall {
                name: SmolStr::new(fn_name),
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
        Ok(())
    }
}
