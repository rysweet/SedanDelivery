//! `coalesce()` and `optional()` (sugar for coalesce).

#![allow(unused_imports)]

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::Operator;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    pub(super) fn compile_coalesce(
        &mut self,
        alternatives: &[Vec<GremlinStep>],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        // Compile each alternative and track their output aliases.
        let mut alt_ops = Vec::new();
        let mut alt_aliases = Vec::new();
        for s in alternatives {
            let mut sub = self.sub_compiler();
            let op = sub.compile_seeded(s, &ctx.current_alias)?;
            alt_aliases.push(sub.last_alias.clone());
            // Propagate edge source so otherV() after coalesce works.
            if sub.edge_src_vertex.is_some() && self.edge_src_vertex.is_none() {
                self.edge_src_vertex = sub.edge_src_vertex.clone();
            }
            self.absorb(&sub);
            alt_ops.push(op);
        }

        // Normalize: wrap each alternative in a Project to rename
        // its output to a common alias.
        let coal_alias = self.next_alias("coal");
        let edge_src = self.edge_src_vertex.clone();
        let normalized: Vec<Operator> = alt_ops
            .into_iter()
            .zip(alt_aliases.iter())
            .map(|(op, alt_alias)| {
                if alt_alias.as_str() == coal_alias.as_str() {
                    op
                } else {
                    let mut items = vec![
                        (Expr::Variable(alt_alias.clone()), Some(coal_alias.clone())),
                        // Also carry forward the original alias for path()
                        (Expr::Variable(alt_alias.clone()), Some(alt_alias.clone())),
                    ];
                    // Carry the edge source vertex so otherV() can
                    // find it in the row after coalesce.
                    if let Some(ref src) = edge_src {
                        items.push((Expr::Variable(src.clone()), Some(src.clone())));
                    }
                    Operator::Project {
                        items,
                        child: Box::new(op),
                    }
                }
            })
            .collect();

        ctx.current = Some(Operator::Coalesce {
            alternatives: normalized,
            child: Box::new(child),
        });
        ctx.current_alias = coal_alias;
        ctx.path_aliases.push(ctx.current_alias.clone());
        self.path_tracking.push(ctx.current_alias.clone());
        Ok(())
    }

    pub(super) fn compile_optional(
        &mut self,
        opt_steps: &[GremlinStep],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // optional(traversal): coalesce(traversal, identity)
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let mut sub = self.sub_compiler();
        let opt_op = sub.compile_seeded(opt_steps, &ctx.current_alias)?;
        self.absorb(&sub);
        // Merge sub-compiler's path tracking into local ctx.path_aliases
        // so path()/simplePath() see intermediate aliases from the
        // optional body (e.g., out() inside optional).
        for alias in &sub.path_tracking {
            if !ctx.path_aliases.contains(alias) {
                ctx.path_aliases.push(alias.clone());
            }
        }
        ctx.current_alias = sub.last_alias.clone();
        ctx.current = Some(Operator::Coalesce {
            alternatives: vec![opt_op, Operator::SingleRow],
            child: Box::new(child),
        });
        if !ctx.path_aliases.contains(&ctx.current_alias) {
            ctx.path_aliases.push(ctx.current_alias.clone());
        }
        if !self.path_tracking.contains(&ctx.current_alias) {
            self.path_tracking.push(ctx.current_alias.clone());
        }
        Ok(())
    }
}
