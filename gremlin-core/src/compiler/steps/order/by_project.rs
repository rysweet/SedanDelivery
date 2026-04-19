//! `by(traversal)` modulator after `project()` — compiles the sub-traversal
//! and converts the pending `Operator::Project` into `Operator::GremlinProject`
//! on first By, then fills in subsequent entries.

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::Operator;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    pub(super) fn apply_project_by_traversal(
        &mut self,
        sub_steps: &[GremlinStep],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // Compile the sub-traversal seeded with the pre-project alias
        let mut sub = self.sub_compiler();
        if self.seed_is_map_entry {
            sub.seed_is_map_entry = true;
        }
        let sub_op = sub.compile_seeded(sub_steps, &ctx.pre_project_alias)?;
        let sub_out_alias = sub.last_alias.clone();
        self.absorb(&sub);

        // Convert Operator::Project → GremlinProject on first ByTraversal
        match &ctx.current {
            Some(Operator::Project { items, child }) => {
                // Build GremlinProject entries from existing items
                let mut entries: Vec<(String, Operator, Identifier, bool)> = Vec::new();
                for (i, (_expr, alias_opt)) in items.iter().enumerate() {
                    if i < ctx.project_keys.len() {
                        if i == ctx.project_by_count {
                            // This is the by(traversal) entry — not identity
                            entries.push((
                                ctx.project_keys[i].clone(),
                                sub_op.clone(),
                                sub_out_alias.clone(),
                                false,
                            ));
                        } else if i < ctx.project_by_count {
                            // Previously filled by By() — check if identity.
                            // If items[i].0 is Expr::Variable it was empty by().
                            let is_ident = matches!(&items[i].0, Expr::Variable(_));
                            let prop_op = Operator::Project {
                                items: vec![(items[i].0.clone(), alias_opt.clone())],
                                child: Box::new(Operator::SingleRow),
                            };
                            entries.push((
                                ctx.project_keys[i].clone(),
                                prop_op,
                                alias_opt
                                    .as_ref()
                                    .cloned()
                                    .unwrap_or_else(|| Identifier::new(&ctx.project_keys[i])),
                                is_ident,
                            ));
                        } else {
                            // Not yet filled — default to property access
                            let prop_op = Operator::Project {
                                items: vec![(items[i].0.clone(), alias_opt.clone())],
                                child: Box::new(Operator::SingleRow),
                            };
                            entries.push((
                                ctx.project_keys[i].clone(),
                                prop_op,
                                alias_opt
                                    .as_ref()
                                    .cloned()
                                    .unwrap_or_else(|| Identifier::new(&ctx.project_keys[i])),
                                false,
                            ));
                        }
                    }
                }
                let proj_alias = self.next_alias("proj");
                ctx.current = Some(Operator::GremlinProject {
                    entries,
                    child: child.clone(),
                    current_alias: ctx.pre_project_alias.clone(),
                    out_alias: proj_alias,
                    productive: self.productive_by,
                });
            }
            Some(Operator::GremlinProject {
                entries: old_entries,
                child,
                current_alias: ca,
                out_alias,
                ..
            }) => {
                let mut entries = old_entries.clone();
                if ctx.project_by_count < entries.len() {
                    entries[ctx.project_by_count] = (
                        ctx.project_keys[ctx.project_by_count].clone(),
                        sub_op,
                        sub_out_alias,
                        false, // by(traversal) — not identity
                    );
                }
                ctx.current = Some(Operator::GremlinProject {
                    entries,
                    child: child.clone(),
                    current_alias: ca.clone(),
                    out_alias: out_alias.clone(),
                    productive: self.productive_by,
                });
            }
            _ => {}
        }
        ctx.project_by_count += 1;
        if ctx.project_by_count >= ctx.project_keys.len() {
            ctx.pending_project = false;
            if let Some(Operator::GremlinProject {
                out_alias: proj_alias,
                ..
            }) = &ctx.current
            {
                ctx.current_alias = proj_alias.clone();
            }
        }
        Ok(())
    }
}
