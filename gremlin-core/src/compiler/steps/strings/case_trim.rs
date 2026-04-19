//! Case-conversion and whitespace-trimming string steps:
//! `ToLower`, `ToUpper`, `Trim`, `LTrim`, `RTrim`.

use cypher_core::ast::Expr;
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};

impl GremlinCompiler {
    pub(super) fn compile_str_to_lower(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        self.emit_unary_str_projection(ctx, "gremlin_toLower", "str")
    }

    pub(super) fn compile_str_to_upper(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        self.emit_unary_str_projection(ctx, "gremlin_toUpper", "str")
    }

    pub(super) fn compile_str_trim(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        self.emit_unary_str_projection(ctx, "gremlin_trim", "str")
    }

    pub(super) fn compile_str_ltrim(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        self.emit_unary_str_projection(ctx, "gremlin_lTrim", "str")
    }

    pub(super) fn compile_str_rtrim(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        self.emit_unary_str_projection(ctx, "gremlin_rTrim", "str")
    }

    /// Shared helper: emits a `Project` operator that calls `fn_name` on the
    /// current alias and assigns the result to a fresh alias seeded with
    /// `alias_seed`.
    pub(super) fn emit_unary_str_projection(
        &mut self,
        ctx: &mut CompileFromContext,
        fn_name: &str,
        alias_seed: &str,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let alias = self.next_alias(alias_seed);
        let mut items = vec![(
            Expr::FunctionCall {
                name: SmolStr::new(fn_name),
                args: vec![Expr::Variable(ctx.current_alias.clone())],
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
