//! Type-conversion / casting style steps:
//! `AsString`, `AsStringLocal`, `AsNumber`, `AsNumberTyped`, `AsBool`,
//! `AsDate`, and `Math` (deferred-evaluation marker).

use cypher_core::ast::Expr;
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};

impl GremlinCompiler {
    pub(super) fn compile_str_as_string(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        self.emit_unary_str_projection(ctx, "gremlin_asstring", "str")
    }

    pub(super) fn compile_str_as_string_local(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        self.emit_unary_str_projection(ctx, "gremlin_asstring_local", "strl")
    }

    pub(super) fn compile_str_as_number(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        self.emit_unary_str_projection(ctx, "gremlin_asnumber", "num")
    }

    pub(super) fn compile_str_as_number_typed(
        &mut self,
        target_type: &str,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let alias = self.next_alias("num");
        let mut items = vec![(
            Expr::FunctionCall {
                name: SmolStr::new("gremlin_asnumber_typed"),
                args: vec![
                    Expr::Variable(ctx.current_alias.clone()),
                    Expr::Str(target_type.to_string().into_boxed_str()),
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
        self.numeric_subtype = Some(target_type.to_string());
        Ok(())
    }

    pub(super) fn compile_str_as_bool(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        self.emit_unary_str_projection(ctx, "gremlin_asBool", "bool")
    }

    pub(super) fn compile_str_math(&mut self, expr_str: &str, ctx: &mut CompileFromContext) {
        // Set ctx.pending_math so that a following by() can resolve variables
        ctx.pending_math = true;
        ctx.pending_math_expr = expr_str.to_string();
        ctx.pre_math_alias = ctx.current_alias.clone();
    }

    pub(super) fn compile_str_as_date(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // asDate() — convert epoch-millis integer to Date value.
        self.emit_unary_str_projection(ctx, "gremlin_asdate", "date")
    }
}
