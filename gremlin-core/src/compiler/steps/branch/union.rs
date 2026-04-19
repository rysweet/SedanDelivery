//! `union()` branch compilation.

#![allow(unused_imports)]

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::Operator;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    pub(super) fn compile_union(
        &mut self,
        branches: &[Vec<GremlinStep>],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().unwrap_or(Operator::SingleRow);
        if branches.is_empty() {
            // union() with no branches → produce no results
            ctx.current = Some(Operator::Filter {
                predicate: Expr::Bool(false),
                child: Box::new(child),
            });
        } else {
            let all_branches_have_source = branches
                .iter()
                .all(|s| Self::steps_have_independent_source(s));

            // Compile each branch and track its result alias.
            let mut branch_ops: Vec<Operator> = Vec::new();
            let mut branch_aliases: Vec<Identifier> = Vec::new();
            let mut branch_path_trackings: Vec<Vec<Identifier>> = Vec::new();
            for s in branches {
                let mut sub = self.sub_compiler();
                let has_source = Self::steps_have_independent_source(s);
                let op = if has_source {
                    sub.compile_seeded(s, &ctx.current_alias)?
                } else {
                    sub.compile_from(
                        s,
                        Some(child.clone()),
                        ctx.current_alias.clone(),
                        ctx.path_aliases.clone(),
                    )?
                };
                branch_aliases.push(sub.last_alias.clone());
                branch_path_trackings.push(sub.path_tracking.clone());
                self.absorb(&sub);
                branch_ops.push(op);
            }

            // Choose a common output alias for the union.
            let union_alias = self.next_alias("u");

            // Wrap each branch in a Project to normalize the output alias.
            let normalized: Vec<Operator> = branch_ops
                .into_iter()
                .zip(branch_aliases.iter())
                .zip(branch_path_trackings.iter())
                .map(|((op, src_alias), branch_paths)| {
                    if src_alias == &union_alias {
                        op
                    } else {
                        let mut items =
                            vec![(Expr::Variable(src_alias.clone()), Some(union_alias.clone()))];
                        // Carry forward all step labels
                        items.extend(self.label_carry_items());

                        // When path tracking is active, build
                        // __gremlin_path_acc from the branch's
                        // intermediate path aliases (all but the
                        // last, which becomes the union output).
                        if self.needs_path_tracking && branch_paths.len() > 1 {
                            let acc_items: Vec<Expr> = branch_paths[..branch_paths.len() - 1]
                                .iter()
                                .map(|a| Expr::Variable(a.clone()))
                                .collect();
                            items.push((
                                Expr::List(acc_items),
                                Some(Identifier::new("__gremlin_path_acc")),
                            ));
                        }

                        Operator::Project {
                            items,
                            child: Box::new(op),
                        }
                    }
                })
                .collect();

            // Build a left-associative union chain
            let union = normalized
                .into_iter()
                .reduce(|left, right| Operator::Union {
                    left: Box::new(left),
                    right: Box::new(right),
                    all: true,
                })
                .unwrap_or(Operator::SingleRow);

            // Gremlin union() operates over the ctx.current traverser
            // stream. Even when that stream is provided through
            // SingleRow/foreach_seed (repeat(), choose(), local()),
            // wrapping the union in Apply would incorrectly force
            // per-row semantics and break barrier steps like
            // limit(), count(), and barrier() inside branches.
            let _ = all_branches_have_source;
            ctx.current = Some(union);
            ctx.current_alias = union_alias;
            ctx.path_aliases.push(ctx.current_alias.clone());
            self.path_tracking.push(ctx.current_alias.clone());
        }
        Ok(())
    }
}
