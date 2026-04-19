//! `by(traversal)` modulators for non-project, non-group contexts:
//! `order`, `dedup`, `aggregate`, `format`, and `sack`.

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::Operator;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::helpers::rewrite_sack_by;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    pub(super) fn apply_order_by_traversal(
        &mut self,
        sub_steps: &[GremlinStep],
        descending: bool,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // by(traversal) after order() — compile sub-traversal for sort
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let mut sub = self.sub_compiler();
        let sub_op = sub.compile_seeded(sub_steps, &ctx.current_alias)?;
        let sub_out_alias = sub.last_alias.clone();
        self.absorb(&sub);
        let applied = Operator::Apply {
            outer: Box::new(child),
            inner: Box::new(sub_op),
        };
        ctx.current = Some(Operator::Sort {
            items: vec![(Expr::Variable(sub_out_alias), !descending)],
            gremlin_order: true,
            child: Box::new(applied),
        });
        ctx.pending_order = false;
        Ok(())
    }

    pub(super) fn apply_dedup_by_traversal(
        &mut self,
        sub_steps: &[GremlinStep],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // by(traversal) after dedup() — compile sub-traversal for dedup key
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let mut sub = self.sub_compiler();
        let sub_op = sub.compile_seeded(sub_steps, &ctx.current_alias)?;
        let sub_out_alias = sub.last_alias.clone();
        self.absorb(&sub);
        let applied = Operator::Apply {
            outer: Box::new(child),
            inner: Box::new(sub_op),
        };
        ctx.current = Some(Operator::Distinct {
            child: Box::new(applied),
            by_key: Some(sub_out_alias.to_string()),
            productive: self.productive_by,
        });
        ctx.pending_dedup = false;
        Ok(())
    }

    pub(super) fn apply_aggregate_by_traversal(
        &mut self,
        sub_steps: &[GremlinStep],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // by(traversal) after aggregate() — compile sub-traversal
        let mut sub = self.sub_compiler();
        let sub_op = sub.compile_seeded(sub_steps, &ctx.current_alias)?;
        let sub_out_alias = sub.last_alias.clone();
        self.absorb(&sub);
        if let Some(Operator::GremlinAggregate {
            ref mut by_traversal,
            ..
        }) = ctx.current
        {
            *by_traversal = Some((Box::new(sub_op), sub_out_alias));
        }
        ctx.pending_aggregate = false;
        ctx.aggregate_by_consumed = true;
        Ok(())
    }

    pub(super) fn apply_format_by_traversal(
        &mut self,
        sub_steps: &[GremlinStep],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // by(traversal) after format() — compile sub-traversal for by-modulation
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let mut sub = self.sub_compiler();
        let sub_op = sub.compile_seeded(sub_steps, &ctx.format_pre_alias)?;
        let sub_out_alias = sub.last_alias.clone();
        self.absorb(&sub);
        // Limit to 1 result per input row
        let limited = Operator::Limit {
            count: Expr::Integer(1),
            child: Box::new(sub_op),
        };
        ctx.current = Some(Operator::Apply {
            outer: Box::new(child),
            inner: Box::new(limited),
        });
        ctx.format_by_aliases.push(sub_out_alias);
        Ok(())
    }

    pub(super) fn apply_sack_by_traversal(
        &mut self,
        sub_steps: &[GremlinStep],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // by(traversal) after sack(op) — rewrite sack assignment
        // to use the sub-traversal result instead of ctx.current_alias.
        let mut sub = self.sub_compiler();
        let sub_op = sub.compile_seeded(sub_steps, &ctx.current_alias)?;
        let sub_out_alias = sub.last_alias.clone();
        self.absorb(&sub);
        let by_expr = Expr::Variable(sub_out_alias);
        // Wrap ctx.current operator in Apply so the sub-traversal runs
        // per-row, then rewrite the sack Project's rhs.
        if let Some(ref mut proj) = ctx.current {
            // First, wrap child in Apply with the sub-traversal
            let sack_var = Identifier::new("__sack");
            if let Operator::Project {
                ref mut items,
                ref mut child,
            } = proj
            {
                **child = Operator::Apply {
                    outer: child.clone(),
                    inner: Box::new(sub_op),
                };
                if let Some((expr, _)) = items.first_mut() {
                    rewrite_sack_by(expr, &ctx.current_alias, &by_expr);
                }
            }
            // Filter out rows where __sack is null
            let proj_taken = std::mem::replace(proj, Operator::SingleRow);
            *proj = Operator::Filter {
                predicate: Expr::IsNotNull(Box::new(Expr::Variable(sack_var))),
                child: Box::new(proj_taken),
            };
        }
        ctx.pending_sack_op = false;
        ctx.pending_sack_by_consumed = true;
        Ok(())
    }
}
