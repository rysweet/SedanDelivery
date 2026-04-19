//! `by(traversal)` modulator after `valueMap()` — wraps the current Project
//! to unfold list values via the `gremlin_valuemap_unfold` function.

use cypher_core::ast::Expr;
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    pub(super) fn apply_valuemap_unfold_by(
        &mut self,
        sub_steps: &[GremlinStep],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let is_unfold = sub_steps.len() == 1 && matches!(sub_steps[0], GremlinStep::Unfold);
        if is_unfold {
            // Wrap the ctx.current valueMap result to unfold lists
            let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
            let alias = self.next_alias("vmuf");
            let mut items = vec![(
                Expr::FunctionCall {
                    name: SmolStr::new("gremlin_valuemap_unfold"),
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
        }
        Ok(())
    }
}
