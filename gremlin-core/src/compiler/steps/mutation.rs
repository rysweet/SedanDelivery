//! `mutation` category step compilation arms extracted from `dispatch.rs`.
//!
//! Pure mechanical extraction — no behavior changes. Each arm body is
//! verbatim from the corresponding case in the original monolithic match.

#![allow(unused_imports)]

use std::collections::HashMap;

use cypher_core::ast::{BinaryOp, Direction, Expr, Identifier, Label, RelType};
use cypher_core::planner::{
    AggFunction, CreateEdgeSpec, CreatePattern, GremlinPop, Operator, SetItem,
};
use smol_str::SmolStr;

use super::super::compile_from_context::CompileFromContext;
use super::super::helpers::*;
use super::super::{CompileError, GremlinCompiler, SelectByModulator};
use super::StepFlow;
use crate::bytecode::{GremlinStep, Predicate};
use crate::types::GremlinValue;

impl GremlinCompiler {
    pub(super) fn compile_mutation_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::AddV(label) => {
                if self.read_only {
                    return Err(CompileError::ReadOnlyViolation);
                }
                ctx.pending_properties.clear();
                let alias = self.next_alias("nv");
                ctx.current_alias = alias.clone();
                let pattern = CreatePattern {
                    nodes: vec![(
                        Some(alias),
                        vec![Label::new(label.as_str())],
                        vec![], // properties added later
                    )],
                    edges: vec![],
                };
                ctx.current = Some(Operator::Create {
                    patterns: vec![pattern],
                    child: ctx.current.take().map(Box::new),
                });
            }

            GremlinStep::AddVTraversal(label_steps) => {
                if self.read_only {
                    return Err(CompileError::ReadOnlyViolation);
                }
                ctx.pending_properties.clear();
                let input = ctx.current.take().unwrap_or(Operator::SingleRow);
                let pre_add_alias = ctx.current_alias.clone();
                let mut sub = self.sub_compiler();
                let label_op = sub.compile_seeded(label_steps, &pre_add_alias)?;
                let label_alias = sub.last_alias.clone();
                self.absorb(&sub);

                let alias = self.next_alias("nv");
                ctx.current_alias = alias.clone();
                let create = Operator::Create {
                    patterns: vec![CreatePattern {
                        nodes: vec![(Some(alias.clone()), vec![], vec![])],
                        edges: vec![],
                    }],
                    child: Some(Box::new(Operator::Apply {
                        outer: Box::new(input),
                        inner: Box::new(label_op),
                    })),
                };
                ctx.current = Some(Operator::Set {
                    items: vec![SetItem::DynamicLabels {
                        alias: alias.clone(),
                        value: Expr::Variable(label_alias),
                    }],
                    child: Box::new(create),
                });
            }

            GremlinStep::AddE(label) => {
                if self.read_only {
                    return Err(CompileError::ReadOnlyViolation);
                }
                ctx.pending_properties.clear();
                ctx.adde_traverser = Some(ctx.current_alias.clone());
                let rel_alias = self.next_alias("ne");
                ctx.current_alias = rel_alias.clone();
                // Placeholder: src/dst to be set by From/To steps
                let src = Identifier::new("__src");
                let dst = Identifier::new("__dst");
                let edge_spec: CreateEdgeSpec = (
                    Some(rel_alias),
                    src,
                    dst,
                    RelType::new(label.as_str()),
                    Direction::Outgoing,
                    vec![],
                );
                let pattern = CreatePattern {
                    nodes: vec![],
                    edges: vec![edge_spec],
                };
                ctx.current = Some(Operator::Create {
                    patterns: vec![pattern],
                    child: ctx.current.take().map(Box::new),
                });
            }

            GremlinStep::MergeV(ref search_map) => {
                if self.read_only {
                    return Err(CompileError::ReadOnlyViolation);
                }
                ctx.pending_properties.clear();
                let alias = self.next_alias("mv");
                ctx.current_alias = alias.clone();

                let mut labels = Vec::new();
                let mut properties = Vec::new();
                if let GremlinValue::Map(ref map) = search_map {
                    for (k, v) in map {
                        if k == "t[label]" || k == "T.label" {
                            if let GremlinValue::String(l) = v {
                                labels.push(Label::new(l.as_str()));
                            }
                        } else if k != "t[id]" && k != "T.id" {
                            properties.push((k.clone(), gremlin_value_to_expr(v)));
                        }
                    }
                }

                ctx.current = Some(Operator::Merge {
                    node_alias: Some(alias),
                    labels,
                    properties,
                    on_create: vec![],
                    on_match: vec![],
                    child: ctx.current.take().map(Box::new),
                });
            }

            GremlinStep::MergeE(ref search_map) => {
                if self.read_only {
                    return Err(CompileError::ReadOnlyViolation);
                }
                ctx.pending_properties.clear();
                ctx.adde_traverser = Some(ctx.current_alias.clone());
                let rel_alias = self.next_alias("me");
                ctx.current_alias = rel_alias.clone();

                let mut rel_type_str = String::new();
                let mut properties = Vec::new();
                if let GremlinValue::Map(ref map) = search_map {
                    for (k, v) in map {
                        if k == "t[label]" || k == "T.label" {
                            if let GremlinValue::String(l) = v {
                                rel_type_str = l.clone();
                            }
                        } else if !k.starts_with('(')
                            && !k.starts_with("Direction.")
                            && k != "t[id]"
                            && k != "T.id"
                        {
                            properties.push((k.clone(), gremlin_value_to_expr(v)));
                        }
                    }
                }

                let src = Identifier::new("__src");
                let dst = Identifier::new("__dst");
                let rel_type = if rel_type_str.is_empty() {
                    RelType::new("__default")
                } else {
                    RelType::new(rel_type_str.as_str())
                };
                ctx.current = Some(Operator::MergeRelationship {
                    src_alias: src,
                    dst_alias: dst,
                    rel_alias: Some(rel_alias),
                    rel_type,
                    direction: Direction::Outgoing,
                    properties,
                    on_create: vec![],
                    on_match: vec![],
                    child: Box::new(ctx.current.take().unwrap_or(Operator::SingleRow)),
                });
            }

            GremlinStep::Property(key, value) => {
                if self.read_only {
                    return Err(CompileError::ReadOnlyViolation);
                }
                // Helper: extract the select key from "__.select("x")" patterns
                let extract_select_key = |s: &str| -> Option<String> {
                    let trimmed = s.trim_start_matches("__.");
                    trimmed
                        .strip_prefix("select(")
                        .and_then(|s| s.strip_suffix(')'))
                        .map(|inner| inner.trim_matches('"').trim_matches('\'').to_string())
                };
                // Resolve property key: if __.select("a") and "a" is a string
                // side-effect, use the string value as the property key name.
                let resolved_key = if let Some(ref sel_key) = extract_select_key(key) {
                    if let Some(GremlinValue::String(s)) =
                        self.side_effect_values.get(sel_key.as_str())
                    {
                        s.clone()
                    } else {
                        key.clone()
                    }
                } else {
                    key.clone()
                };
                if let Some(ref reserved) = self.reserved_keys {
                    let default_reserved = ["id", "label"];
                    if default_reserved.contains(&resolved_key.as_str())
                        || reserved.contains(&resolved_key)
                    {
                        return Err(CompileError::ReservedKeyViolation(resolved_key));
                    }
                }
                ctx.pending_properties
                    .push((resolved_key.clone(), value.clone()));
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                // Resolve property value: if __.select("a"), emit Variable ref
                let val_expr = if let GremlinValue::String(s) = value {
                    if let Some(ref sel_key) = extract_select_key(s) {
                        if let Some(alias) = self.alias_map.get(sel_key.as_str()) {
                            Expr::Variable(alias.clone())
                        } else {
                            gremlin_value_to_expr(value)
                        }
                    } else {
                        gremlin_value_to_expr(value)
                    }
                } else {
                    gremlin_value_to_expr(value)
                };
                ctx.current = Some(Operator::Set {
                    items: vec![SetItem::Property {
                        alias: ctx.current_alias.clone(),
                        key: resolved_key,
                        value: val_expr,
                    }],
                    child: Box::new(child),
                });
            }

            GremlinStep::From(_) | GremlinStep::To(_) => {
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
                            if let Ok(inner_op) =
                                sub.compile_seeded(inner_steps, &ctx.current_alias)
                            {
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
            }

            GremlinStep::Drop => {
                if self.read_only {
                    return Err(CompileError::ReadOnlyViolation);
                }
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                ctx.current = Some(Operator::Delete {
                    exprs: vec![Expr::Variable(ctx.current_alias.clone())],
                    detach: true,
                    child: Box::new(child),
                });
            }

            _ => unreachable!(
                "compile_mutation_step received non-mutation variant: {:?}",
                step
            ),
        }
        #[allow(unreachable_code)]
        Ok(StepFlow::Proceed)
    }
}
