//! `by` step variant: by() after project() — replace the default property expression for the next project key.
//!
//! Pure mechanical extraction from the original `by.rs` — no behavior changes.

#![allow(unused_imports)]

use std::collections::HashMap;

use cypher_core::ast::{BinaryOp, Direction, Expr, Identifier, Label, RelType};
use cypher_core::planner::{
    AggFunction, CreateEdgeSpec, CreatePattern, GremlinPop, Operator, SetItem,
};
use smol_str::SmolStr;

use crate::bytecode::{GremlinStep, Predicate};
use crate::compiler::compile_from_context::CompileFromContext;
use crate::compiler::helpers::*;
use crate::compiler::{CompileError, GremlinCompiler, SelectByModulator};
use crate::types::GremlinValue;

impl GremlinCompiler {
    pub(super) fn handle_by_project(
        &mut self,
        key: &str,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // by() after project() — replace the default property
        // expression for the next project key with the by() expression.
        if ctx.project_by_count < ctx.project_keys.len() {
            // Check if the key looks like a sub-traversal
            let looks_like_traversal = key.contains('(') && !key.is_empty();
            if looks_like_traversal {
                // Parse and compile the sub-traversal
                let no_param = |_: &str| -> Option<GremlinValue> { None };
                let sub_steps = crate::text_parser::parse_inner_traversal(key, &no_param)
                    .map_err(|e| CompileError::InvalidSequence(e.to_string()))?;
                if !sub_steps.is_empty() {
                    let mut sub = self.sub_compiler();
                    let sub_op = sub.compile_seeded(&sub_steps, &ctx.pre_project_alias)?;
                    let sub_out_alias = sub.last_alias.clone();
                    self.absorb(&sub);

                    // Convert to GremlinProject
                    match &ctx.current {
                        Some(Operator::Project { items, child }) => {
                            let mut entries: Vec<(String, Operator, Identifier, bool)> = Vec::new();
                            for (i, (expr, alias_opt)) in items.iter().enumerate() {
                                if i >= ctx.project_keys.len() {
                                    break;
                                }
                                if i == ctx.project_by_count {
                                    // by(traversal) — not identity
                                    entries.push((
                                        ctx.project_keys[i].clone(),
                                        sub_op.clone(),
                                        sub_out_alias.clone(),
                                        false,
                                    ));
                                } else {
                                    let prop_op = Operator::Project {
                                        items: vec![(expr.clone(), alias_opt.clone())],
                                        child: Box::new(Operator::SingleRow),
                                    };
                                    // Prior entries may have been set by
                                    // empty by() (identity). We can't
                                    // recover that info here since it was
                                    // stored in items[i].0 type. Assume
                                    // non-identity for safety.
                                    entries.push((
                                        ctx.project_keys[i].clone(),
                                        prop_op,
                                        alias_opt.as_ref().cloned().unwrap_or_else(|| {
                                            Identifier::new(&ctx.project_keys[i])
                                        }),
                                        false,
                                    ));
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
                } else {
                    // Empty by() steps — use simple property expression.
                    let by_expr = make_by_expr(key, &ctx.pre_project_alias);
                    if let Some(Operator::Project { ref mut items, .. }) = ctx.current {
                        if let Some((expr, _)) = items.get_mut(ctx.project_by_count) {
                            *expr = by_expr;
                        }
                    }
                    ctx.project_by_count += 1;
                    if ctx.project_by_count >= ctx.project_keys.len() {
                        ctx.pending_project = false;
                    }
                }
            } else {
                // Simple property key
                let by_expr = if key.is_empty() {
                    Expr::Variable(ctx.pre_project_alias.clone())
                } else {
                    make_by_expr(key, &ctx.pre_project_alias)
                };
                if let Some(Operator::Project { ref mut items, .. }) = ctx.current {
                    if let Some((expr, _)) = items.get_mut(ctx.project_by_count) {
                        *expr = by_expr;
                    }
                } else if let Some(Operator::GremlinProject {
                    ref mut entries, ..
                }) = ctx.current
                {
                    // Already converted to GremlinProject — update the entry
                    if ctx.project_by_count < entries.len() {
                        let is_identity = key.is_empty();
                        let by_ex = if is_identity {
                            Expr::Variable(ctx.pre_project_alias.clone())
                        } else {
                            make_by_expr(key, &ctx.pre_project_alias)
                        };
                        let prop_op = Operator::Project {
                            items: vec![(
                                by_ex,
                                Some(Identifier::new(&ctx.project_keys[ctx.project_by_count])),
                            )],
                            child: Box::new(Operator::SingleRow),
                        };
                        entries[ctx.project_by_count] = (
                            ctx.project_keys[ctx.project_by_count].clone(),
                            prop_op,
                            Identifier::new(&ctx.project_keys[ctx.project_by_count]),
                            is_identity,
                        );
                    }
                }
                ctx.project_by_count += 1;
                if ctx.project_by_count >= ctx.project_keys.len() {
                    ctx.pending_project = false;
                    // If GremlinProject, update ctx.current_alias to its out_alias
                    if let Some(Operator::GremlinProject {
                        out_alias: ref proj_alias,
                        ..
                    }) = ctx.current
                    {
                        ctx.current_alias = proj_alias.clone();
                    }
                    // When all by() modulators are simple property keys, the
                    // operator stays as Operator::Project, which does NOT
                    // assemble the projected Map under ctx.current_alias.
                    // Add a map-assembly wrapper so downstream steps (e.g.
                    // merge(), combine()) receive a Map as the traverser
                    // instead of the original Node.  GremlinProject already
                    // handles this for traversal-based by().
                    if matches!(&ctx.current, Some(Operator::Project { .. }))
                        && !ctx.project_keys.is_empty()
                    {
                        let proj_alias = self.next_alias("proj");
                        let map_expr = Expr::MapLiteral(
                            ctx.project_keys
                                .iter()
                                .map(|k| {
                                    (
                                        SmolStr::new(k.as_str()),
                                        Expr::Variable(Identifier::new(k.as_str())),
                                    )
                                })
                                .collect(),
                        );
                        let inner = ctx.current.take().unwrap();
                        // Carry existing label aliases first, then
                        // place the map assembly last so it wins.
                        let mut items = self.label_carry_items();
                        items.push((map_expr, Some(proj_alias.clone())));
                        ctx.current = Some(Operator::Project {
                            items,
                            child: Box::new(inner),
                        });
                        ctx.current_alias = proj_alias;
                    }
                }
            }
        }
        Ok(())
    }
}
