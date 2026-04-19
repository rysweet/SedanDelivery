//! `from()` / `to()` step compilation — edge endpoint and path modifier.

#![allow(unused_imports)]

use cypher_core::ast::{BinaryOp, Expr, Identifier};
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::helpers::*;
use super::super::super::{CompileError, GremlinCompiler};
use super::super::StepFlow;
use crate::bytecode::GremlinStep;
use crate::types::GremlinValue;

impl GremlinCompiler {
    pub(super) fn compile_from_to(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        // Edge endpoint modifiers OR path from/to modifiers.
        let inner_steps = match step {
            GremlinStep::From(s) | GremlinStep::To(s) => s.as_slice(),
            _ => unreachable!(),
        };
        let is_from = matches!(step, GremlinStep::From(_));

        // Check if this is a path/simplePath/cyclicPath modifier (from/to with a label string).
        let is_path_modifier = matches!(
            &ctx.current,
            Some(Operator::GremlinPath { .. })
                | Some(Operator::GremlinSimplePath { .. })
                | Some(Operator::GremlinCyclicPath { .. })
        );

        if is_path_modifier && inner_steps.len() == 1 {
            if let GremlinStep::Select(_, keys) = &inner_steps[0] {
                if keys.len() == 1 {
                    // Resolve user-facing label to internal alias
                    // so the executor can match against ctx.path_aliases.
                    let label = self
                        .alias_map
                        .get(&keys[0])
                        .map(|id| id.as_str().to_string())
                        .unwrap_or_else(|| keys[0].clone());
                    match &mut ctx.current {
                        Some(Operator::GremlinPath {
                            from_label,
                            to_label,
                            ..
                        }) => {
                            if is_from {
                                *from_label = Some(label);
                            } else {
                                *to_label = Some(label);
                            }
                        }
                        Some(Operator::GremlinSimplePath {
                            from_label,
                            to_label,
                            ..
                        }) => {
                            if is_from {
                                *from_label = Some(label);
                            } else {
                                *to_label = Some(label);
                            }
                        }
                        Some(Operator::GremlinCyclicPath {
                            from_label,
                            to_label,
                            ..
                        }) => {
                            if is_from {
                                *from_label = Some(label);
                            } else {
                                *to_label = Some(label);
                            }
                        }
                        _ => {}
                    }
                    return Ok(StepFlow::Continue);
                }
            }
            // Bare string label reference: from("a")
            if let GremlinStep::Constant(GremlinValue::String(label)) = &inner_steps[0] {
                // Resolve user-facing label to internal alias
                let resolved = self
                    .alias_map
                    .get(label)
                    .map(|id| id.as_str().to_string())
                    .unwrap_or_else(|| label.clone());
                match &mut ctx.current {
                    Some(Operator::GremlinPath {
                        from_label,
                        to_label,
                        ..
                    }) => {
                        if is_from {
                            *from_label = Some(resolved.clone());
                        } else {
                            *to_label = Some(resolved.clone());
                        }
                    }
                    Some(Operator::GremlinSimplePath {
                        from_label,
                        to_label,
                        ..
                    }) => {
                        if is_from {
                            *from_label = Some(resolved.clone());
                        } else {
                            *to_label = Some(resolved.clone());
                        }
                    }
                    Some(Operator::GremlinCyclicPath {
                        from_label,
                        to_label,
                        ..
                    }) => {
                        if is_from {
                            *from_label = Some(resolved.clone());
                        } else {
                            *to_label = Some(resolved.clone());
                        }
                    }
                    _ => {}
                }
                return Ok(StepFlow::Continue);
            }
        }

        // Resolve the vertex reference from the inner steps
        let resolved_alias = if inner_steps.len() == 1 {
            match &inner_steps[0] {
                GremlinStep::V(Some(ids)) if ids.len() == 1 => {
                    // from(V(id)) or to(V(id)) — compile a node lookup
                    let alias = self.next_alias("fv");
                    let scan = Operator::Filter {
                        predicate: Expr::BinaryOp {
                            op: BinaryOp::Eq,
                            left: Box::new(Expr::FunctionCall {
                                name: SmolStr::new("id"),
                                args: vec![Expr::Variable(alias.clone())],
                                distinct: false,
                            }),
                            right: Box::new(gremlin_value_to_id_expr(&ids[0])),
                        },
                        child: Box::new(Operator::NodeScan {
                            label: None,
                            alias: alias.clone(),
                        }),
                    };
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    // Inject the vertex lookup as a cross join into the
                    // Create's child chain so that the fv_N alias is
                    // present in every row when Create resolves src/dst.
                    ctx.current = Some(inject_scan_into_child(child, scan));
                    alias
                }
                GremlinStep::Select(_, keys) if keys.len() == 1 => {
                    // from(select("a")) or to(select("a"))
                    self.alias_map
                        .get(&keys[0])
                        .cloned()
                        .unwrap_or_else(|| Identifier::new(keys[0].clone()))
                }
                _ => {
                    // Single-step complex traversal
                    let mut sub = self.sub_compiler();
                    if let Ok(inner_op) = sub.compile_seeded(inner_steps, &ctx.current_alias) {
                        self.absorb(&sub);
                        let inner_alias = sub.last_alias.clone();
                        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                        ctx.current = Some(inject_scan_into_child(child, inner_op));
                        inner_alias
                    } else {
                        ctx.current_alias.clone()
                    }
                }
            }
        } else {
            // Multi-step traversal in from/to
            let mut sub = self.sub_compiler();
            if let Ok(inner_op) = sub.compile_seeded(inner_steps, &ctx.current_alias) {
                self.absorb(&sub);
                let inner_alias = sub.last_alias.clone();
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                ctx.current = Some(inject_scan_into_child(child, inner_op));
                inner_alias
            } else {
                ctx.current_alias.clone()
            }
        };

        // Rewrite the __src/__dst placeholder in the Create operator
        let placeholder = if is_from { "__src" } else { "__dst" };
        if let Some(ref mut op) = ctx.current {
            rewrite_create_alias(op, placeholder, &resolved_alias);
        }
        Ok(StepFlow::Proceed)
    }
}
