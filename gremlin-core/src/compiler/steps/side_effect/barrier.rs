//! `none()`, `barrier()`, `barrierNormSack()`, `discard()`, and `loops()` step compilation.

#![allow(unused_imports)]

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::Operator;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};

impl GremlinCompiler {
    pub(super) fn compile_none(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // none() — produces no results (empty)
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        ctx.current = Some(Operator::Filter {
            predicate: Expr::Bool(false),
            child: Box::new(child),
        });
        Ok(())
    }

    pub(super) fn compile_barrier(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // barrier() without sack merge: synchronization no-op.
        // Only generate GremlinBarrier when sack merge is active.
        if ctx.pending_sack_merge_op.is_some() {
            let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
            let ca = ctx.current_alias.clone();
            let out = self.next_alias("barrier");
            ctx.current = Some(Operator::GremlinBarrier {
                child: Box::new(child),
                current_alias: ca.clone(),
                out_alias: out.clone(),
                norm_sack: false,
                sack_merge_op: ctx.pending_sack_merge_op.clone(),
                with_bulk: !ctx.with_bulk_false,
            });
            ctx.current_alias = out;
        } else if ctx.current.is_none() {
            return Err(CompileError::MissingChild);
        }
        Ok(())
    }

    pub(super) fn compile_barrier_norm_sack(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let ca = ctx.current_alias.clone();
        let out = self.next_alias("barrier");
        ctx.current = Some(Operator::GremlinBarrier {
            child: Box::new(child),
            current_alias: ca.clone(),
            out_alias: out.clone(),
            norm_sack: true,
            sack_merge_op: ctx.pending_sack_merge_op.clone(),
            with_bulk: !ctx.with_bulk_false,
        });
        ctx.current_alias = out;
        Ok(())
    }

    pub(super) fn compile_discard(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // discard() — consume traversal, return empty
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        ctx.current = Some(Operator::Filter {
            predicate: Expr::Bool(false),
            child: Box::new(child),
        });
        Ok(())
    }

    pub(super) fn compile_loops(&mut self, label: &Option<String>, ctx: &mut CompileFromContext) {
        // loops() or loops("label") — returns ctx.current loop iteration counter.
        // Inside a repeat body the executor injects __gremlin_loop_count
        // (unlabeled) or __gremlin_loop_count_<label> (labeled).
        let child = ctx.current.take().unwrap_or(Operator::SingleRow);
        let alias = self.next_alias("loops");
        let var_name = match label {
            Some(l) => format!("__gremlin_loop_count_{l}"),
            None => "__gremlin_loop_count".to_string(),
        };
        let loop_var = Identifier::new(&var_name);
        let mut items = vec![(Expr::Variable(loop_var), Some(alias.clone()))];
        items.extend(self.label_carry_items());
        ctx.current = Some(Operator::Project {
            items,
            child: Box::new(child),
        });
        ctx.current_alias = alias;
    }
}
