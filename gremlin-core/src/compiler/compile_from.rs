//! `GremlinCompiler::compile_from` — the central per-step compilation routine.
//!
//! Extracted from `mod.rs` to keep that file focused on struct definition and
//! small helpers; this file holds the giant left-to-right step walker that
//! compiles a `Vec<GremlinStep>` into an `Operator` tree.

use std::collections::HashMap;

use cypher_core::ast::{BinaryOp, Direction, Expr, Identifier, Label, RelType};
use cypher_core::planner::{
    AggFunction, CreateEdgeSpec, CreatePattern, GremlinPop, Operator, SetItem,
};
use smol_str::SmolStr;

use super::compile_from_context::CompileFromContext;
use super::helpers::*;
use super::{CompileError, GremlinCompiler, SelectByModulator};
use crate::bytecode::{GremlinStep, Predicate};
use crate::types::GremlinValue;

impl GremlinCompiler {
    /// Core compilation: walks steps left-to-right, wrapping each into the
    /// operator tree. `initial_op` / `initial_alias` allow callers to seed
    /// the compilation with an existing operator and alias.
    pub(super) fn compile_from(
        &mut self,
        steps: &[GremlinStep],
        initial_op: Option<Operator>,
        initial_alias: Identifier,
        initial_path_aliases: Vec<Identifier>,
    ) -> Result<Operator, CompileError> {
        if steps.is_empty() {
            return Ok(initial_op.unwrap_or(Operator::SingleRow));
        }

        // Preprocess infix and()/or() with no arguments into proper branching
        // forms — unless ConnectiveStrategy was explicitly disabled.
        let has_without_connective = steps
            .iter()
            .any(|s| matches!(s, GremlinStep::WithoutConnectiveStrategy));
        let preprocessed = if has_without_connective {
            steps.to_vec()
        } else {
            preprocess_infix_connectives(steps)
        };
        let steps = &preprocessed;

        // Pre-scan: detect if any step uses path tracking (path, simplePath, cyclicPath).
        // This enables carrying path aliases through Project operations.
        if !self.needs_path_tracking {
            self.needs_path_tracking = steps_need_path_tracking(steps);
        }
        // Pre-scan: detect if any select() uses Pop modifiers.
        if !self.needs_label_stacks {
            self.needs_label_stacks = steps_need_label_stacks(steps);
        }
        // Pre-scan: collect aggregate() keys so that within()/without() predicates
        // that reference a key defined LATER in the same step sequence (e.g. inside
        // repeat() bodies) can resolve to __gremlin_agg_<key> at compile time.
        collect_aggregate_keys_prescan(steps, &mut self.aggregate_keys);

        let mut ctx = CompileFromContext::new(
            initial_op,
            initial_alias.clone(),
            initial_path_aliases,
        );
        // Sync path_tracking from initial_path_aliases
        self.path_tracking = ctx.path_aliases.clone();

        for step in steps {
            // Save alias before step to detect changes for path tracking
            let pre_step_alias = ctx.current_alias.clone();
            // Clear numeric subtype on value-changing steps; Is/As/filter steps preserve it.
            if !matches!(
                step,
                GremlinStep::Is(_)
                    | GremlinStep::As(_)
                    | GremlinStep::By(..)
                    | GremlinStep::ByTraversal(..)
            ) {
                self.numeric_subtype = None;
            }
            // When we leave the addE/mergeE modifier group (from/to/property/option),
            // fill in any __src/__dst placeholders that were never overridden.
            if ctx.adde_traverser.is_some()
                && !matches!(
                    step,
                    GremlinStep::From(_)
                        | GremlinStep::To(_)
                        | GremlinStep::Property(..)
                        | GremlinStep::Option(..)
                )
            {
                if let Some(ref traverser) = ctx.adde_traverser.take() {
                    if let Some(ref mut op) = ctx.current {
                        rewrite_create_alias(op, "__src", traverser);
                        rewrite_create_alias(op, "__dst", traverser);
                    }
                }
            }
            // Reset aggregate by-consumed flag on any non-By step
            if !matches!(step, GremlinStep::By(..) | GremlinStep::ByTraversal(..)) {
                ctx.aggregate_by_consumed = false;
                ctx.dedup_by_consumed = false;
                // dedup/group/aggregate/format by() modulators must immediately follow
                // their step. Any intervening non-By step terminates the pending state.
                ctx.pending_dedup = false;
                ctx.pending_aggregate = false;
                ctx.pending_format = false;
                ctx.pending_sack_op = false;
                ctx.pending_sack_by_consumed = false;
                ctx.pending_valuemap = false;
                ctx.valuemap_by_consumed = false;
                ctx.pending_group = false;
                ctx.pending_group_is_count = false;
                ctx.pending_group_by_count = 0;
                // Flush accumulated path by-keys into the GremlinPath/CyclicPath operator
                if ctx.pending_path && !ctx.path_by_keys.is_empty() {
                    match &mut ctx.current {
                        Some(Operator::GremlinPath {
                            ref mut by_keys, ..
                        }) => {
                            *by_keys = ctx.path_by_keys.clone();
                        }
                        Some(Operator::GremlinCyclicPath {
                            ref mut by_keys, ..
                        }) => {
                            *by_keys = ctx.path_by_keys.clone();
                        }
                        Some(Operator::GremlinSimplePath {
                            ref mut by_keys, ..
                        }) => {
                            *by_keys = ctx.path_by_keys.clone();
                        }
                        _ => {}
                    }
                }
                // Preserve ctx.pending_path through from()/to() modifiers so
                // subsequent by() modulations are still accumulated.
                let is_path_from_to = matches!(step, GremlinStep::From(_) | GremlinStep::To(_))
                    && matches!(
                        &ctx.current,
                        Some(Operator::GremlinPath { .. })
                            | Some(Operator::GremlinSimplePath { .. })
                            | Some(Operator::GremlinCyclicPath { .. })
                    );
                if !is_path_from_to {
                    ctx.pending_path = false;
                    ctx.path_by_keys.clear();
                }
                ctx.pending_project = false;
                if !ctx.pending_where_by_targets.is_empty() && !ctx.pending_where_by_modulators.is_empty() {
                    if let Some(Operator::Filter {
                        ref mut predicate, ..
                    }) = ctx.current
                    {
                        finalize_where_by_modulation(
                            predicate,
                            &ctx.pending_where_by_targets,
                            &ctx.pending_where_by_modulators,
                            self.productive_by,
                        );
                    }
                }
                ctx.pending_where_by_targets.clear();
                ctx.pending_where_by_modulators.clear();
                // Flush pending math without by() modulation — resolve side-effect variables
                if ctx.pending_math {
                    let child = ctx.current.take().unwrap_or(Operator::SingleRow);
                    let alias = self.next_alias("math");
                    let mut args = vec![
                        Expr::Variable(ctx.pre_math_alias.clone()),
                        Expr::Str(ctx.pending_math_expr.clone().into_boxed_str()),
                    ];
                    // Resolve variable references in the expression from side effects
                    for var_name in extract_math_variables(&ctx.pending_math_expr) {
                        let se_key = format!("__side_effect_{}", var_name);
                        if let Some(se_ident) = self.alias_map.get(&var_name) {
                            args.push(Expr::Str(var_name.into_boxed_str()));
                            args.push(Expr::Variable(se_ident.clone()));
                        } else if self.side_effect_values.contains_key(&var_name) {
                            args.push(Expr::Str(var_name.into_boxed_str()));
                            args.push(Expr::Variable(Identifier::new(se_key)));
                        }
                    }
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_math"),
                            args,
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
                    ctx.pending_math = false;
                    ctx.pending_math_expr.clear();
                }
                // Flush pending order: order() without by() → sort by identity
                if ctx.pending_order && !ctx.pending_order_local {
                    let child = ctx.current.take().unwrap_or(Operator::SingleRow);
                    ctx.current = Some(Operator::Sort {
                        items: vec![(Expr::Variable(ctx.current_alias.clone()), true)],
                        gremlin_order: true,
                        child: Box::new(child),
                    });
                    ctx.pending_order = false;
                }
                // Flush pending order(Scope.local) without by() — sort list by natural ordering
                if ctx.pending_order && ctx.pending_order_local {
                    let child = ctx.current.take().unwrap_or(Operator::SingleRow);
                    let alias = self.next_alias("srtl");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_sort_local"),
                            args: vec![
                                Expr::Variable(ctx.current_alias.clone()),
                                Expr::Str("".into()),
                                Expr::Str("asc".into()),
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
                    ctx.pending_order = false;
                    ctx.pending_order_local = false;
                }
                // Finalize pending sample once the optional by() window has closed.
                if ctx.pending_sample {
                    let child = ctx.current.take().unwrap_or(Operator::SingleRow);
                    ctx.current = Some(Operator::GremlinSample {
                        count: ctx.pending_sample_count as usize,
                        weight: ctx.pending_sample_weight.clone(),
                        seed: self.seed,
                        child: Box::new(child),
                    });
                    ctx.pending_sample = false;
                    ctx.sample_by_consumed = false;
                    ctx.pending_sample_count = 0;
                    ctx.pending_sample_weight = None;
                }
                // Finalize pending format with collected by() modulations
                if ctx.pending_format && !ctx.format_by_aliases.is_empty() {
                    let child = ctx.current.take().unwrap_or(Operator::SingleRow);
                    let alias = self.next_alias("fmt");
                    // Build gremlin_format_by args: pattern, then each by-alias value
                    let mut args = vec![
                        Expr::Variable(ctx.format_pre_alias.clone()),
                        Expr::Str(ctx.format_pattern.clone().into_boxed_str()),
                    ];
                    for by_alias in &ctx.format_by_aliases {
                        args.push(Expr::Variable(by_alias.clone()));
                    }
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_format_by"),
                            args,
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    let project = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(project),
                    });
                    ctx.current_alias = alias;
                    ctx.format_by_aliases.clear();
                }
                ctx.pending_format = false;
                // Apply collected select by() modulations cyclically
                if ctx.pending_select && !ctx.select_by_mods.is_empty() {
                    self.apply_pending_select_mods(
                        &mut ctx.current,
                        &mut ctx.current_alias,
                        &ctx.select_by_mods,
                    )?;
                }
                ctx.pending_select = false;
            }
            // Flush pending choose output alias on the first non-Option step
            if !matches!(step, GremlinStep::Option(..)) {
                if let Some(alias) = ctx.pending_choose_output.take() {
                    ctx.current_alias = alias;
                }
            }
            match step {
                GremlinStep::V(ids) => {
                    let alias = self.next_alias("v");
                    ctx.path_aliases.push(alias.clone());
                    self.path_tracking.push(alias.clone());
                    let scan = Operator::NodeScan {
                        label: None,
                        alias: alias.clone(),
                    };
                    // Flatten list parameters: V(listParam) → V(id1, id2, ...)
                    let flat_ids: Option<Vec<&GremlinValue>> = ids.as_ref().map(|id_list| {
                        let mut flat = Vec::new();
                        for id in id_list {
                            if let GremlinValue::List(inner) = id {
                                flat.extend(inner.iter());
                            } else {
                                flat.push(id);
                            }
                        }
                        flat
                    });
                    let new_op = match &flat_ids {
                        Some(id_list) if id_list.len() == 1 => {
                            // Use id-aware conversion: string "1" → Integer(1)
                            let id_expr = gremlin_value_to_id_expr(id_list[0]);
                            Operator::Filter {
                                predicate: Expr::BinaryOp {
                                    op: BinaryOp::Eq,
                                    left: Box::new(Expr::FunctionCall {
                                        name: SmolStr::new("id"),
                                        args: vec![Expr::Variable(alias.clone())],
                                        distinct: false,
                                    }),
                                    right: Box::new(id_expr),
                                },
                                child: Box::new(scan),
                            }
                        }
                        Some(id_list) if !id_list.is_empty() => {
                            let id_exprs: Vec<Expr> = id_list
                                .iter()
                                .map(|v| gremlin_value_to_id_expr(v))
                                .collect();
                            Operator::Filter {
                                predicate: Expr::BinaryOp {
                                    op: BinaryOp::In,
                                    left: Box::new(Expr::FunctionCall {
                                        name: SmolStr::new("id"),
                                        args: vec![Expr::Variable(alias.clone())],
                                        distinct: false,
                                    }),
                                    right: Box::new(Expr::List(id_exprs)),
                                },
                                child: Box::new(scan),
                            }
                        }
                        _ => scan,
                    };
                    // Mid-traversal V(): cross-join with existing rows
                    if let Some(child) = ctx.current.take() {
                        ctx.current = Some(Operator::Apply {
                            outer: Box::new(child),
                            inner: Box::new(new_op),
                        });
                    } else {
                        ctx.current = Some(new_op);
                    }
                    ctx.current_alias = alias;
                }

                GremlinStep::E(ids) => {
                    // Edge scan: NodeScan → Expand (all types) → optional id filter
                    let src = self.next_alias("src");
                    let rel_alias = self.next_alias("e");
                    let dst = self.next_alias("dst");
                    let expand = Operator::Expand {
                        direction: Direction::Outgoing,
                        rel_types: vec![],
                        src_alias: src.clone(),
                        rel_alias: Some(rel_alias.clone()),
                        dst_alias: dst,
                        child: Box::new(Operator::NodeScan {
                            label: None,
                            alias: src,
                        }),
                    };
                    ctx.current_alias = rel_alias.clone();
                    ctx.path_aliases.push(ctx.current_alias.clone());
                    self.path_tracking.push(ctx.current_alias.clone());
                    // Flatten list parameters: E(listParam) → E(id1, id2, ...)
                    let flat_ids: Option<Vec<&GremlinValue>> = ids.as_ref().map(|id_list| {
                        let mut flat = Vec::new();
                        for id in id_list {
                            if let GremlinValue::List(inner) = id {
                                flat.extend(inner.iter());
                            } else {
                                flat.push(id);
                            }
                        }
                        flat
                    });
                    let new_op = match &flat_ids {
                        Some(id_list) if id_list.len() == 1 => {
                            // Use id-aware conversion: string "11" → Integer(11)
                            let id_expr = gremlin_value_to_id_expr(id_list[0]);
                            Operator::Filter {
                                predicate: Expr::BinaryOp {
                                    op: BinaryOp::Eq,
                                    left: Box::new(Expr::FunctionCall {
                                        name: SmolStr::new("id"),
                                        args: vec![Expr::Variable(rel_alias)],
                                        distinct: false,
                                    }),
                                    right: Box::new(id_expr),
                                },
                                child: Box::new(expand),
                            }
                        }
                        Some(id_list) if !id_list.is_empty() => {
                            let id_exprs: Vec<Expr> = id_list
                                .iter()
                                .map(|v| gremlin_value_to_id_expr(v))
                                .collect();
                            Operator::Filter {
                                predicate: Expr::BinaryOp {
                                    op: BinaryOp::In,
                                    left: Box::new(Expr::FunctionCall {
                                        name: SmolStr::new("id"),
                                        args: vec![Expr::Variable(rel_alias)],
                                        distinct: false,
                                    }),
                                    right: Box::new(Expr::List(id_exprs)),
                                },
                                child: Box::new(expand),
                            }
                        }
                        _ => expand,
                    };
                    // Mid-traversal E(): cross-join with existing rows
                    if let Some(child) = ctx.current.take() {
                        ctx.current = Some(Operator::Apply {
                            outer: Box::new(child),
                            inner: Box::new(new_op),
                        });
                    } else {
                        ctx.current = Some(new_op);
                    }
                }

                GremlinStep::HasLabel(labels) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    if labels.is_empty() {
                        // hasLabel() with no labels — no filtering needed
                        ctx.current = Some(child);
                    } else if labels.len() == 1 {
                        ctx.current = Some(Operator::Filter {
                            predicate: Expr::HasLabel(
                                Box::new(Expr::Variable(ctx.current_alias.clone())),
                                Label::new(&labels[0]),
                            ),
                            child: Box::new(child),
                        });
                    } else {
                        // OR of label checks
                        let mut checks: Vec<Expr> = labels
                            .iter()
                            .map(|l| {
                                Expr::HasLabel(
                                    Box::new(Expr::Variable(ctx.current_alias.clone())),
                                    Label::new(l.as_str()),
                                )
                            })
                            .collect();
                        let predicate = checks
                            .drain(..)
                            .reduce(|a, b| Expr::BinaryOp {
                                op: BinaryOp::Or,
                                left: Box::new(a),
                                right: Box::new(b),
                            })
                            .unwrap();
                        ctx.current = Some(Operator::Filter {
                            predicate,
                            child: Box::new(child),
                        });
                    }
                }

                GremlinStep::Has(key, value) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let prop_expr = gremlin_has_key_expr(key, &ctx.current_alias);
                    let predicate = match value {
                        Some(val) => Expr::BinaryOp {
                            op: BinaryOp::Eq,
                            left: Box::new(prop_expr),
                            right: Box::new(if is_gremlin_id_key(key) {
                                gremlin_value_to_id_expr(val)
                            } else {
                                gremlin_value_to_expr(val)
                            }),
                        },
                        None => Expr::IsNotNull(Box::new(prop_expr)),
                    };
                    ctx.current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(child),
                    });
                }

                GremlinStep::HasPredicate(key, pred) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let prop_expr = gremlin_has_key_expr(key, &ctx.current_alias);
                    let predicate = predicate_to_prop_expr(pred, prop_expr);
                    ctx.current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(child),
                    });
                }

                GremlinStep::HasId(ids) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let id_fn = Expr::FunctionCall {
                        name: SmolStr::new("id"),
                        args: vec![Expr::Variable(ctx.current_alias.clone())],
                        distinct: false,
                    };
                    // Flatten list parameters: hasId(id1, [id2, id3]) → hasId(id1, id2, id3).
                    // Also filter out null IDs: hasId(id, null) — TinkerPop semantics treat
                    // null as "no match" so null values are silently dropped from the id set.
                    let mut flat_ids = Vec::new();
                    for id in ids {
                        match id {
                            GremlinValue::List(inner) => {
                                flat_ids.extend(
                                    inner
                                        .iter()
                                        .filter(|v| !matches!(v, GremlinValue::Null))
                                        .cloned(),
                                );
                            }
                            GremlinValue::Null => {} // skip null ids
                            other => flat_ids.push(other.clone()),
                        }
                    }
                    let predicate = if flat_ids.len() == 1 {
                        Expr::BinaryOp {
                            op: BinaryOp::Eq,
                            left: Box::new(id_fn),
                            // Use id-aware conversion: string "1" → Integer(1)
                            right: Box::new(gremlin_value_to_id_expr(&flat_ids[0])),
                        }
                    } else {
                        let id_exprs: Vec<Expr> =
                            flat_ids.iter().map(gremlin_value_to_id_expr).collect();
                        Expr::BinaryOp {
                            op: BinaryOp::In,
                            left: Box::new(id_fn),
                            right: Box::new(Expr::List(id_exprs)),
                        }
                    };
                    ctx.current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(child),
                    });
                }

                GremlinStep::HasIdPredicate(pred) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let id_fn = Expr::FunctionCall {
                        name: SmolStr::new("id"),
                        args: vec![Expr::Variable(ctx.current_alias.clone())],
                        distinct: false,
                    };
                    let predicate = predicate_to_prop_expr(pred, id_fn);
                    ctx.current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(child),
                    });
                }

                GremlinStep::Out(labels) | GremlinStep::In(labels) | GremlinStep::Both(labels) => {
                    if self.edge_label_verify && labels.is_empty() {
                        return Err(CompileError::EdgeLabelRequired);
                    }
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let dst = self.next_alias("v");
                    let direction = match step {
                        GremlinStep::Out(_) => Direction::Outgoing,
                        GremlinStep::In(_) => Direction::Incoming,
                        _ => Direction::Undirected,
                    };
                    let rel_types: Vec<RelType> =
                        labels.iter().map(|l| RelType::new(l.as_str())).collect();
                    ctx.current = Some(Operator::Expand {
                        direction,
                        rel_types,
                        src_alias: ctx.current_alias.clone(),
                        rel_alias: None,
                        dst_alias: dst.clone(),
                        child: Box::new(child),
                    });
                    ctx.current_alias = dst;
                    ctx.path_aliases.push(ctx.current_alias.clone());
                    self.path_tracking.push(ctx.current_alias.clone());
                }

                GremlinStep::OutE(labels)
                | GremlinStep::InE(labels)
                | GremlinStep::BothE(labels) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let rel_alias = self.next_alias("e");
                    let dst = self.next_alias("v");
                    let direction = match step {
                        GremlinStep::OutE(_) => Direction::Outgoing,
                        GremlinStep::InE(_) => Direction::Incoming,
                        _ => Direction::Undirected,
                    };
                    let rel_types: Vec<RelType> =
                        labels.iter().map(|l| RelType::new(l.as_str())).collect();
                    let edge_src = ctx.current_alias.clone();
                    ctx.current = Some(Operator::Expand {
                        direction,
                        rel_types,
                        src_alias: ctx.current_alias.clone(),
                        rel_alias: Some(rel_alias.clone()),
                        dst_alias: dst,
                        child: Box::new(child),
                    });
                    ctx.current_alias = rel_alias;
                    self.edge_src_vertex = Some(edge_src);
                    ctx.path_aliases.push(ctx.current_alias.clone());
                    self.path_tracking.push(ctx.current_alias.clone());
                }

                GremlinStep::InV | GremlinStep::OutV => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let vertex_alias = self.next_alias("v");
                    let fn_name = match step {
                        GremlinStep::InV => "endNode",
                        _ => "startNode",
                    };
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new(fn_name),
                            args: vec![Expr::Variable(ctx.current_alias.clone())],
                            distinct: false,
                        },
                        Some(vertex_alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    ctx.current_alias = vertex_alias;
                    ctx.path_aliases.push(ctx.current_alias.clone());
                    self.path_tracking.push(ctx.current_alias.clone());
                }

                GremlinStep::OtherV => {
                    // otherV: return the vertex that is NOT the traversal source.
                    // Pass both the edge and the source vertex so the runtime can
                    // determine which endpoint is "other".
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let vertex_alias = self.next_alias("v");
                    let src_arg = self
                        .edge_src_vertex
                        .clone()
                        .map(Expr::Variable)
                        .unwrap_or(Expr::Null);
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_other_v"),
                            args: vec![Expr::Variable(ctx.current_alias.clone()), src_arg],
                            distinct: false,
                        },
                        Some(vertex_alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    ctx.current_alias = vertex_alias;
                    ctx.path_aliases.push(ctx.current_alias.clone());
                    self.path_tracking.push(ctx.current_alias.clone());
                }

                GremlinStep::BothV => {
                    // bothV: return BOTH source and destination vertices.
                    // Build a list [startNode, endNode] and unwind.
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let list_alias = self.next_alias("bv");
                    let vertex_alias = self.next_alias("v");
                    let carry = self.label_carry_items();
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_both_v"),
                            args: vec![Expr::Variable(ctx.current_alias.clone())],
                            distinct: false,
                        },
                        Some(list_alias.clone()),
                    )];
                    items.extend(carry);
                    let project = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    ctx.current = Some(Operator::Unwind {
                        list_expr: Expr::Variable(list_alias),
                        alias: vertex_alias.clone(),
                        child: Box::new(project),
                    });
                    ctx.current_alias = vertex_alias;
                    ctx.path_aliases.push(ctx.current_alias.clone());
                    self.path_tracking.push(ctx.current_alias.clone());
                }

                GremlinStep::Values(keys) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let carry = self.label_carry_items();
                    if keys.len() == 1 {
                        // Single key: project to a named alias, filter out nulls
                        // (Gremlin values() drops traversers without the property)
                        let alias = self.next_alias("val");
                        let mut items = vec![(
                            Expr::Property(
                                Box::new(Expr::Variable(ctx.current_alias.clone())),
                                SmolStr::new(&keys[0]),
                            ),
                            Some(alias.clone()),
                        )];
                        items.extend(carry);
                        let project = Operator::Project {
                            items,
                            child: Box::new(child),
                        };
                        ctx.current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                            child: Box::new(project),
                        });
                        ctx.current_alias = alias;
                    } else if keys.is_empty() {
                        // values() with no keys: unwind all property values
                        let alias = self.next_alias("val");
                        let mut items = vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_values_all"),
                                args: vec![Expr::Variable(ctx.current_alias.clone())],
                                distinct: false,
                            },
                            Some(alias.clone()),
                        )];
                        items.extend(carry);
                        let project = Operator::Project {
                            items,
                            child: Box::new(child),
                        };
                        // Unwind the list of all values
                        let uf_alias = self.next_alias("uf");
                        ctx.current = Some(Operator::Unwind {
                            list_expr: Expr::Variable(alias),
                            alias: uf_alias.clone(),
                            child: Box::new(project),
                        });
                        ctx.current_alias = uf_alias;
                    } else {
                        // Multiple keys: collect values for each key into a list, then unwind
                        let list_alias = self.next_alias("vlist");
                        let mut fn_args = vec![Expr::Variable(ctx.current_alias.clone())];
                        for k in keys {
                            fn_args.push(Expr::Str(k.as_str().into()));
                        }
                        let mut items = vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_values_keys"),
                                args: fn_args,
                                distinct: false,
                            },
                            Some(list_alias.clone()),
                        )];
                        items.extend(carry);
                        let project = Operator::Project {
                            items,
                            child: Box::new(child),
                        };
                        let uf_alias = self.next_alias("uf");
                        ctx.current = Some(Operator::Unwind {
                            list_expr: Expr::Variable(list_alias),
                            alias: uf_alias.clone(),
                            child: Box::new(project),
                        });
                        ctx.current_alias = uf_alias;
                    }
                }

                GremlinStep::ValueMap(keys, token_mode) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("vmap");
                    let mut args = vec![
                        Expr::Variable(ctx.current_alias.clone()),
                        Expr::Integer(i64::from(*token_mode)),
                    ];
                    for k in keys {
                        args.push(Expr::Str(k.as_str().into()));
                    }
                    let fc = Expr::FunctionCall {
                        name: SmolStr::new("gremlin_valuemap"),
                        args,
                        distinct: false,
                    };
                    let mut items = vec![(fc, Some(alias.clone()))];
                    items.extend(self.label_carry_items());
                    ctx.current_alias = alias;
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    ctx.pending_valuemap = true;
                    ctx.valuemap_by_consumed = false;
                }

                GremlinStep::Properties(keys) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let prop_alias = self.next_alias("prop");
                    let carry = self.label_carry_items();
                    if keys.is_empty() {
                        // properties() with no keys: unwind all properties as
                        // Map entries with ~key, ~value, ~element fields.
                        let list_alias = self.next_alias("plist");
                        let mut items = vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_properties_entries"),
                                args: vec![Expr::Variable(ctx.current_alias.clone())],
                                distinct: false,
                            },
                            Some(list_alias.clone()),
                        )];
                        items.extend(carry);
                        let project = Operator::Project {
                            items,
                            child: Box::new(child),
                        };
                        ctx.current = Some(Operator::Unwind {
                            list_expr: Expr::Variable(list_alias),
                            alias: prop_alias.clone(),
                            child: Box::new(project),
                        });
                    } else if keys.len() == 1 {
                        // Single key: produce a property Map with ~key, ~value, ~element.
                        let list_alias = self.next_alias("plist");
                        let mut items = vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_properties_entries"),
                                args: vec![
                                    Expr::Variable(ctx.current_alias.clone()),
                                    Expr::Str(keys[0].as_str().into()),
                                ],
                                distinct: false,
                            },
                            Some(list_alias.clone()),
                        )];
                        items.extend(carry);
                        let project = Operator::Project {
                            items,
                            child: Box::new(child),
                        };
                        ctx.current = Some(Operator::Unwind {
                            list_expr: Expr::Variable(list_alias),
                            alias: prop_alias.clone(),
                            child: Box::new(project),
                        });
                    } else {
                        // Multiple keys: build property entry list for each key and unwind
                        let list_alias = self.next_alias("plist");
                        let mut args = vec![Expr::Variable(ctx.current_alias.clone())];
                        for k in keys {
                            args.push(Expr::Str(k.as_str().into()));
                        }
                        let mut items = vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_properties_entries"),
                                args,
                                distinct: false,
                            },
                            Some(list_alias.clone()),
                        )];
                        items.extend(carry);
                        let project = Operator::Project {
                            items,
                            child: Box::new(child),
                        };
                        ctx.current = Some(Operator::Unwind {
                            list_expr: Expr::Variable(list_alias),
                            alias: prop_alias.clone(),
                            child: Box::new(project),
                        });
                    }
                    ctx.current_alias = prop_alias;
                }

                GremlinStep::Id => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let id_alias = self.next_alias("id");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("id"),
                            args: vec![Expr::Variable(ctx.current_alias.clone())],
                            distinct: false,
                        },
                        Some(id_alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    ctx.current_alias = id_alias;
                }

                GremlinStep::Label => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let lbl_alias = self.next_alias("lbl");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_single_label"),
                            args: vec![Expr::Variable(ctx.current_alias.clone())],
                            distinct: false,
                        },
                        Some(lbl_alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    ctx.current_alias = lbl_alias;
                }

                GremlinStep::As(alias) => {
                    self.alias_map.insert(alias.clone(), ctx.current_alias.clone());
                    self.label_stacks
                        .entry(alias.clone())
                        .or_default()
                        .push(ctx.current_alias.clone());
                }

                GremlinStep::Select(pop, keys) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;

                    // Handle Column.keys / Column.values special selects
                    if keys.len() == 1
                        && (keys[0] == "Column.keys"
                            || keys[0] == "Column.values"
                            || keys[0] == "keys"
                            || keys[0] == "values")
                    {
                        // Special case: path().select(Column.keys) — return the
                        // path's step label sets as a List of Lists.  Each entry
                        // is the sorted set of user labels for that path step;
                        // steps with no labels are omitted (TinkerPop semantics).
                        let is_keys = keys[0] == "Column.keys" || keys[0] == "keys";

                        // Use Option so we can conditionally consume `child`
                        // without Rust thinking it's always moved.
                        let mut child_opt = Some(child);

                        let handled_as_path_keys = if is_keys {
                            if let Some(Operator::GremlinPath {
                                ref path_aliases, ..
                            }) = child_opt
                            {
                                // Invert alias_map: internal_alias → [user_label, ...]
                                let mut alias_to_labels: HashMap<&str, Vec<&str>> = HashMap::new();
                                for (label, alias) in &self.alias_map {
                                    alias_to_labels
                                        .entry(alias.as_str())
                                        .or_default()
                                        .push(label.as_str());
                                }
                                let label_sets: Vec<Expr> = path_aliases
                                    .iter()
                                    .filter_map(|alias| {
                                        let mut lbls: Vec<&str> = alias_to_labels
                                            .get(alias.as_str())
                                            .cloned()
                                            .unwrap_or_default();
                                        if lbls.is_empty() {
                                            None
                                        } else {
                                            lbls.sort_unstable();
                                            Some(Expr::MapLiteral(vec![(
                                                SmolStr::new("~set"),
                                                Expr::List(
                                                    lbls.iter()
                                                        .map(|l| {
                                                            Expr::Str(
                                                                l.to_string().into_boxed_str(),
                                                            )
                                                        })
                                                        .collect(),
                                                ),
                                            )]))
                                        }
                                    })
                                    .collect();
                                let alias = self.next_alias("path_keys");
                                ctx.current = Some(Operator::Project {
                                    items: vec![(Expr::List(label_sets), Some(alias.clone()))],
                                    child: Box::new(child_opt.take().unwrap()),
                                });
                                ctx.current_alias = alias;
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        };

                        if !handled_as_path_keys {
                            let child = child_opt.take().unwrap();
                            let fn_name = if matches!(&child, Operator::Unwind { .. })
                                || self.seed_is_map_entry
                            {
                                if keys[0] == "Column.keys" || keys[0] == "keys" {
                                    "gremlin_unfold_map_entry_key"
                                } else {
                                    "gremlin_unfold_map_entry_value"
                                }
                            } else if keys[0] == "Column.keys" || keys[0] == "keys" {
                                "gremlin_column_key"
                            } else {
                                "gremlin_select_values"
                            };
                            let alias = self.next_alias(fn_name);
                            let mut items = vec![(
                                Expr::FunctionCall {
                                    name: SmolStr::new(fn_name),
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
                        } // end if !handled_as_path_keys
                    } else {
                        let select_keys: Vec<(Identifier, String)> = keys
                            .iter()
                            .map(|k| {
                                let var = if let Some(label) = extract_select_traversal_label(k) {
                                    let key_alias = self
                                        .alias_map
                                        .get(&label)
                                        .cloned()
                                        .unwrap_or_else(|| Identifier::new(label));
                                    Identifier::new(format!(
                                        "__map_select__{}__keyalias__{}",
                                        ctx.current_alias.0, key_alias.0
                                    ))
                                } else {
                                    self.alias_map
                                        .get(k)
                                        .cloned()
                                        .unwrap_or_else(|| Identifier::new(k.clone()))
                                };
                                (var, k.clone())
                            })
                            .collect();
                        // Build label stacks for Pop support
                        let stacks: Vec<(String, Vec<Identifier>)> = keys
                            .iter()
                            .map(|k| {
                                let stack = self.label_stacks.get(k).cloned().unwrap_or_default();
                                (k.clone(), stack)
                            })
                            .collect();
                        // Map bytecode Pop to planner GremlinPop
                        let planner_pop = pop.map(|p| match p {
                            crate::bytecode::Pop::First => GremlinPop::First,
                            crate::bytecode::Pop::Last => GremlinPop::Last,
                            crate::bytecode::Pop::All => GremlinPop::All,
                            crate::bytecode::Pop::Mixed => GremlinPop::Mixed,
                        });
                        // Update ctx.current_alias if single key select
                        if keys.len() == 1 {
                            ctx.current_alias = select_keys[0].0.clone();
                        } else {
                            // Multi-key: set alias to the Map output column name
                            ctx.current_alias = Identifier::new(keys.join("_"));
                        }
                        ctx.current = Some(Operator::GremlinSelect {
                            keys: select_keys,
                            by_traversals: Vec::new(),
                            productive: self.productive_by,
                            pop: planner_pop,
                            label_stacks: stacks,
                            child: Box::new(child),
                        });
                        ctx.pending_select = true;
                        ctx.select_by_mods.clear();
                    }
                }

                GremlinStep::Count => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let count_alias = self.next_alias("cnt");
                    ctx.current = Some(Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            AggFunction::Count,
                            Expr::Variable(ctx.current_alias.clone()),
                            None,
                            false,
                            count_alias.clone(),
                        )],
                        child: Box::new(child),
                    });
                    ctx.current_alias = count_alias;
                }

                GremlinStep::Sum => {
                    // Use Fold + SumLocal for Gremlin semantics (empty → empty)
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let fold_alias = self.next_alias("fold");
                    let folded = Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            AggFunction::Collect,
                            Expr::Variable(ctx.current_alias.clone()),
                            None,
                            false,
                            fold_alias.clone(),
                        )],
                        child: Box::new(child),
                    };
                    let sum_alias = self.next_alias("sum");
                    let projected = Operator::Project {
                        items: vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_sum_local"),
                                args: vec![Expr::Variable(fold_alias)],
                                distinct: false,
                            },
                            Some(sum_alias.clone()),
                        )],
                        child: Box::new(folded),
                    };
                    // Filter out null (empty list produces null sum) unless
                    // ProductiveByStrategy is active (all-null lists → null).
                    if self.productive_by {
                        ctx.current = Some(projected);
                    } else {
                        ctx.current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(sum_alias.clone()))),
                            child: Box::new(projected),
                        });
                    }
                    ctx.current_alias = sum_alias;
                }

                GremlinStep::Min => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("min");
                    let agg = Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            AggFunction::Min,
                            Expr::Variable(ctx.current_alias.clone()),
                            None,
                            false,
                            alias.clone(),
                        )],
                        child: Box::new(child),
                    };
                    if self.productive_by {
                        ctx.current = Some(agg);
                    } else {
                        ctx.current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                            child: Box::new(agg),
                        });
                    }
                    ctx.current_alias = alias;
                }

                GremlinStep::Max => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("max");
                    let agg = Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            AggFunction::Max,
                            Expr::Variable(ctx.current_alias.clone()),
                            None,
                            false,
                            alias.clone(),
                        )],
                        child: Box::new(child),
                    };
                    if self.productive_by {
                        ctx.current = Some(agg);
                    } else {
                        ctx.current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                            child: Box::new(agg),
                        });
                    }
                    ctx.current_alias = alias;
                }

                GremlinStep::Mean => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("avg");
                    let agg = Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            AggFunction::Avg,
                            Expr::Variable(ctx.current_alias.clone()),
                            None,
                            false,
                            alias.clone(),
                        )],
                        child: Box::new(child),
                    };
                    if self.productive_by {
                        ctx.current = Some(agg);
                    } else {
                        ctx.current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                            child: Box::new(agg),
                        });
                    }
                    ctx.current_alias = alias;
                }

                GremlinStep::SumLocal => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("suml");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_sum_local"),
                            args: vec![Expr::Variable(ctx.current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    let proj = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    if self.productive_by {
                        ctx.current = Some(proj);
                    } else {
                        ctx.current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                            child: Box::new(proj),
                        });
                    }
                    ctx.current_alias = alias;
                }

                GremlinStep::MinLocal => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("minl");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_min_local"),
                            args: vec![Expr::Variable(ctx.current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    let proj = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    if self.productive_by {
                        ctx.current = Some(proj);
                    } else {
                        ctx.current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                            child: Box::new(proj),
                        });
                    }
                    ctx.current_alias = alias;
                }

                GremlinStep::MaxLocal => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("maxl");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_max_local"),
                            args: vec![Expr::Variable(ctx.current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    let proj = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    if self.productive_by {
                        ctx.current = Some(proj);
                    } else {
                        ctx.current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                            child: Box::new(proj),
                        });
                    }
                    ctx.current_alias = alias;
                }

                GremlinStep::MeanLocal => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("meanl");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_mean_local"),
                            args: vec![Expr::Variable(ctx.current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    let proj = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    if self.productive_by {
                        ctx.current = Some(proj);
                    } else {
                        ctx.current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                            child: Box::new(proj),
                        });
                    }
                    ctx.current_alias = alias;
                }

                GremlinStep::Group(ref key) | GremlinStep::GroupCount(ref key) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("grp");
                    let is_count = matches!(step, GremlinStep::GroupCount(..));
                    let agg_fn = if is_count {
                        AggFunction::Count
                    } else {
                        AggFunction::Collect
                    };
                    let has_side_effect = key.is_some();
                    // Save the pre-group alias for by() modulation
                    ctx.pre_group_alias = ctx.current_alias.clone();
                    ctx.current = Some(Operator::GremlinGroup {
                        key_expr: Expr::Variable(ctx.current_alias.clone()),
                        value_expr: Expr::Variable(ctx.current_alias.clone()),
                        agg_fn,
                        alias: alias.clone(),
                        side_effect_key: key.clone(),
                        key_traversal: None,
                        value_traversal: None,
                        productive: self.productive_by,
                        key_is_identity: true,
                        explicit_fold: false,
                        child: Box::new(child),
                    });
                    // Named groups are side-effects (pass-through): keep ctx.current alias.
                    // Unnamed groups consume the stream: switch to group alias.
                    if !has_side_effect {
                        ctx.current_alias = alias;
                    }
                    ctx.pending_group = true;
                    ctx.pending_group_by_count = 0;
                    ctx.pending_group_is_count = is_count;
                }

                GremlinStep::Fold => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("fold");
                    ctx.current = Some(Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            AggFunction::CollectAll,
                            Expr::Variable(ctx.current_alias.clone()),
                            None,
                            false,
                            alias.clone(),
                        )],
                        child: Box::new(child),
                    });
                    ctx.current_alias = alias;
                }

                GremlinStep::FoldSeed(seed, op_name) => {
                    // fold(seed, Operator) — collect then reduce with a function.
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let collect_alias = self.next_alias("fold");
                    let agg = Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            AggFunction::Collect,
                            Expr::Variable(ctx.current_alias.clone()),
                            None,
                            false,
                            collect_alias.clone(),
                        )],
                        child: Box::new(child),
                    };
                    let alias = self.next_alias("fred");
                    let op_str = op_name
                        .strip_prefix("Operator.")
                        .unwrap_or(op_name)
                        .to_lowercase();
                    let fc = Expr::FunctionCall {
                        name: SmolStr::new("gremlin_fold_reduce"),
                        args: vec![
                            Expr::Variable(collect_alias),
                            gremlin_value_to_expr(seed),
                            Expr::Str(op_str.into_boxed_str()),
                        ],
                        distinct: false,
                    };
                    let mut items = vec![(fc, Some(alias.clone()))];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(agg),
                    });
                    ctx.current_alias = alias;
                }

                GremlinStep::Unfold => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("uf");
                    ctx.current = Some(Operator::Unwind {
                        list_expr: Expr::Variable(ctx.current_alias.clone()),
                        alias: alias.clone(),
                        child: Box::new(child),
                    });
                    ctx.current_alias = alias;
                    // After unfold, subsequent sub-traversals operate on
                    // map entries — mark so select(Column.values) uses
                    // the direct-extraction function.
                    self.seed_is_map_entry = true;
                }

                GremlinStep::Order => {
                    ctx.pending_order = true;
                    // Actual Sort operator created when we see By (or at the end)
                }

                GremlinStep::By(key, descending) => {
                    if ctx.pending_math {
                        // by() after math() — resolve variable references with property key
                        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                        let alias = self.next_alias("math");
                        let mut args = vec![
                            Expr::Variable(ctx.pre_math_alias.clone()),
                            Expr::Str(ctx.pending_math_expr.clone().into_boxed_str()),
                        ];
                        // For each variable in the expression, resolve using the by key
                        for var_name in extract_math_variables(&ctx.pending_math_expr) {
                            if let Some(stacks) = self.label_stacks.get(&var_name) {
                                if let Some(label_alias) = stacks.last() {
                                    args.push(Expr::Str(var_name.into_boxed_str()));
                                    if key.is_empty() {
                                        args.push(Expr::Variable(label_alias.clone()));
                                    } else {
                                        args.push(Expr::Property(
                                            Box::new(Expr::Variable(label_alias.clone())),
                                            SmolStr::new(key),
                                        ));
                                    }
                                }
                            } else if let Some(se_ident) = self.alias_map.get(&var_name) {
                                args.push(Expr::Str(var_name.into_boxed_str()));
                                if key.is_empty() {
                                    args.push(Expr::Variable(se_ident.clone()));
                                } else {
                                    args.push(Expr::Property(
                                        Box::new(Expr::Variable(se_ident.clone())),
                                        SmolStr::new(key),
                                    ));
                                }
                            }
                        }
                        let mut items = vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_math"),
                                args,
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
                        ctx.pending_math = false;
                        ctx.pending_math_expr.clear();
                    } else if ctx.pending_order && ctx.pending_order_local {
                        // by() after order(Scope.local) — sort within list by key
                        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                        let alias = self.next_alias("srtl");
                        let by_key = if key.is_empty() { "" } else { key };
                        let desc_str = if *descending { "desc" } else { "asc" };
                        let mut items = vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_sort_local"),
                                args: vec![
                                    Expr::Variable(ctx.current_alias.clone()),
                                    Expr::Str(by_key.to_string().into_boxed_str()),
                                    Expr::Str(desc_str.to_string().into_boxed_str()),
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
                        ctx.pending_order = false;
                        ctx.pending_order_local = false;
                    } else if ctx.pending_order {
                        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                        if key.is_empty() {
                            // by() with no key — sort by ctx.current value
                            ctx.current = Some(Operator::Sort {
                                items: vec![(Expr::Variable(ctx.current_alias.clone()), !descending)],
                                gremlin_order: true,
                                child: Box::new(child),
                            });
                        } else if key == "~id" || key == "T.id" || key == "t[id]" {
                            let sort_expr = Expr::FunctionCall {
                                name: SmolStr::new("id"),
                                args: vec![Expr::Variable(ctx.current_alias.clone())],
                                distinct: false,
                            };
                            ctx.current = Some(Operator::Sort {
                                items: vec![(sort_expr, !descending)],
                                gremlin_order: true,
                                child: Box::new(child),
                            });
                        } else if key == "~label" || key == "T.label" || key == "t[label]" {
                            let sort_expr = Expr::FunctionCall {
                                name: SmolStr::new("labels"),
                                args: vec![Expr::Variable(ctx.current_alias.clone())],
                                distinct: false,
                            };
                            ctx.current = Some(Operator::Sort {
                                items: vec![(sort_expr, !descending)],
                                gremlin_order: true,
                                child: Box::new(child),
                            });
                        } else if key == "~key" || key == "T.key" || key == "t[key]" {
                            // Sort by property key name (for VertexProperty traversers)
                            let sort_expr = Expr::FunctionCall {
                                name: SmolStr::new("gremlin_property_key"),
                                args: vec![Expr::Variable(ctx.current_alias.clone())],
                                distinct: false,
                            };
                            ctx.current = Some(Operator::Sort {
                                items: vec![(sort_expr, !descending)],
                                gremlin_order: true,
                                child: Box::new(child),
                            });
                        } else if key == "~value" || key == "T.value" || key == "t[value]" {
                            // Sort by property value (for VertexProperty traversers)
                            let sort_expr = Expr::FunctionCall {
                                name: SmolStr::new("gremlin_property_value"),
                                args: vec![Expr::Variable(ctx.current_alias.clone())],
                                distinct: false,
                            };
                            ctx.current = Some(Operator::Sort {
                                items: vec![(sort_expr, !descending)],
                                gremlin_order: true,
                                child: Box::new(child),
                            });
                        } else if key == "Column.values" {
                            // Sort by map entry values (for unfold'd single-entry Maps)
                            let sort_expr = Expr::FunctionCall {
                                name: SmolStr::new("gremlin_column_values"),
                                args: vec![Expr::Variable(ctx.current_alias.clone())],
                                distinct: false,
                            };
                            ctx.current = Some(Operator::Sort {
                                items: vec![(sort_expr, !descending)],
                                gremlin_order: true,
                                child: Box::new(child),
                            });
                        } else if key == "Column.keys" {
                            // Sort by map entry keys (for unfold'd single-entry Maps)
                            let sort_expr = Expr::FunctionCall {
                                name: SmolStr::new("gremlin_column_keys"),
                                args: vec![Expr::Variable(ctx.current_alias.clone())],
                                distinct: false,
                            };
                            ctx.current = Some(Operator::Sort {
                                items: vec![(sort_expr, !descending)],
                                gremlin_order: true,
                                child: Box::new(child),
                            });
                        } else {
                            // Property-based sort
                            let prop_expr = Expr::Property(
                                Box::new(Expr::Variable(ctx.current_alias.clone())),
                                SmolStr::new(key),
                            );
                            if self.productive_by {
                                // ProductiveByStrategy: use coalesce to return Null
                                // for missing properties instead of filtering.
                                let safe_prop = Expr::FunctionCall {
                                    name: SmolStr::new("coalesce"),
                                    args: vec![prop_expr, Expr::Null],
                                    distinct: false,
                                };
                                ctx.current = Some(Operator::Sort {
                                    items: vec![(safe_prop, !descending)],
                                    gremlin_order: true,
                                    child: Box::new(child),
                                });
                            } else {
                                // Filter out elements missing the property
                                let filtered = Operator::Filter {
                                    predicate: Expr::IsNotNull(Box::new(prop_expr.clone())),
                                    child: Box::new(child),
                                };
                                ctx.current = Some(Operator::Sort {
                                    items: vec![(prop_expr, !descending)],
                                    gremlin_order: true,
                                    child: Box::new(filtered),
                                });
                            }
                        }
                        ctx.pending_order = false;
                    } else if !ctx.pending_order && matches!(ctx.current, Some(Operator::Sort { .. })) {
                        // Additional by() after a Sort — add secondary sort key
                        let sort_expr = if key.is_empty() {
                            Expr::Variable(ctx.current_alias.clone())
                        } else {
                            let prop = Expr::Property(
                                Box::new(Expr::Variable(ctx.current_alias.clone())),
                                SmolStr::new(key),
                            );
                            if self.productive_by {
                                Expr::FunctionCall {
                                    name: SmolStr::new("coalesce"),
                                    args: vec![prop, Expr::Null],
                                    distinct: false,
                                }
                            } else {
                                prop
                            }
                        };
                        if let Some(Operator::Sort { ref mut items, .. }) = ctx.current {
                            items.push((sort_expr, !descending));
                        }
                    } else if ctx.pending_group {
                        // by() after group()/groupCount() — modifies GremlinGroup.
                        // Use ctx.pre_group_alias (the alias before group() changed it).
                        if ctx.pending_group_by_count == 0 {
                            // First by() = group key
                            if !key.is_empty() {
                                // A non-empty key means the group key is no longer the identity.
                                if let Some(Operator::GremlinGroup {
                                    ref mut key_is_identity,
                                    ..
                                }) = ctx.current
                                {
                                    *key_is_identity = false;
                                }
                                // Try parsing __.prefix strings as sub-traversals
                                if key.starts_with("__.") && key.contains('(') {
                                    let no_param = |_: &str| -> Option<GremlinValue> { None };
                                    if let Ok(sub_steps) =
                                        crate::text_parser::parse_inner_traversal(key, &no_param)
                                    {
                                        if !sub_steps.is_empty() {
                                            let mut sub = self.sub_compiler();
                                            if let Ok(sub_op) =
                                                sub.compile_seeded(&sub_steps, &ctx.pre_group_alias)
                                            {
                                                let sub_out_alias = sub.last_alias.clone();
                                                self.absorb(&sub);
                                                if let Some(Operator::GremlinGroup {
                                                    ref mut key_traversal,
                                                    ..
                                                }) = ctx.current
                                                {
                                                    *key_traversal =
                                                        Some((Box::new(sub_op), sub_out_alias));
                                                }
                                            }
                                        }
                                    }
                                } else if let Some(Operator::GremlinGroup {
                                    ref mut key_expr,
                                    ..
                                }) = ctx.current
                                {
                                    *key_expr = make_by_expr(key, &ctx.pre_group_alias);
                                }
                            }
                            ctx.pending_group_by_count = 1;
                        } else if ctx.pending_group_is_count {
                            // groupCount() only supports one by() modulator
                            return Err(CompileError::InvalidSequence(
                                "GroupCount step can only have one by modulator".into(),
                            ));
                        } else {
                            // Second by() = value aggregation
                            if key == "count()" || key == "__.count()" {
                                if let Some(Operator::GremlinGroup { ref mut agg_fn, .. }) = ctx.current
                                {
                                    *agg_fn = AggFunction::Count;
                                }
                            } else if key == "fold()" || key == "__.fold()" {
                                if let Some(Operator::GremlinGroup {
                                    ref mut agg_fn,
                                    ref mut explicit_fold,
                                    ..
                                }) = ctx.current
                                {
                                    *agg_fn = AggFunction::Collect;
                                    *explicit_fold = true;
                                }
                            } else if key == "sum()" || key == "__.sum()" {
                                if let Some(Operator::GremlinGroup { ref mut agg_fn, .. }) = ctx.current
                                {
                                    *agg_fn = AggFunction::Sum;
                                }
                            } else if key.is_empty() {
                                // by() with no args — identity (collect the element)
                                if let Some(Operator::GremlinGroup { ref mut agg_fn, .. }) = ctx.current
                                {
                                    *agg_fn = AggFunction::Collect;
                                }
                            } else if key.starts_with("__.") && key.contains('(') {
                                // __.prefix value-by: parse as sub-traversal
                                let no_param = |_: &str| -> Option<GremlinValue> { None };
                                if let Ok(sub_steps) =
                                    crate::text_parser::parse_inner_traversal(key, &no_param)
                                {
                                    if !sub_steps.is_empty() {
                                        let (proj_steps, trailing_agg) =
                                            split_trailing_group_value_agg(&sub_steps);
                                        if proj_steps.is_empty() {
                                            if let Some(Operator::GremlinGroup {
                                                ref mut agg_fn,
                                                ref mut explicit_fold,
                                                ..
                                            }) = ctx.current
                                            {
                                                if let Some(agg) = trailing_agg {
                                                    if matches!(agg, AggFunction::Collect) {
                                                        *explicit_fold = true;
                                                    }
                                                    *agg_fn = agg;
                                                }
                                            }
                                        } else {
                                            let mut sub = self.sub_compiler();
                                            if let Ok(sub_op) =
                                                sub.compile_seeded(proj_steps, &ctx.pre_group_alias)
                                            {
                                                let sub_out_alias = sub.last_alias.clone();
                                                self.absorb(&sub);
                                                if let Some(Operator::GremlinGroup {
                                                    ref mut value_traversal,
                                                    ref mut agg_fn,
                                                    ref mut explicit_fold,
                                                    ..
                                                }) = ctx.current
                                                {
                                                    *value_traversal =
                                                        Some((Box::new(sub_op), sub_out_alias));
                                                    if let Some(agg) = trailing_agg {
                                                        if matches!(agg, AggFunction::Collect) {
                                                            *explicit_fold = true;
                                                        }
                                                        *agg_fn = agg;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            } else if let Some(Operator::GremlinGroup {
                                ref mut value_expr, ..
                            }) = ctx.current
                            {
                                // by("propName") — collect property values
                                *value_expr = make_by_expr(key, &ctx.pre_group_alias);
                            }
                            ctx.pending_group = false;
                        }
                    } else if ctx.pending_select {
                        // by() after select() — normalize traversal-form
                        // modulators like by(__.math("_+_")) into compact keys.
                        if key.starts_with("__.") && key.contains('(') {
                            let no_param = |_: &str| -> Option<GremlinValue> { None };
                            if let Ok(sub_steps) =
                                crate::text_parser::parse_inner_traversal(key, &no_param)
                            {
                                ctx.select_by_mods.push(SelectByModulator::Key(
                                    extract_by_traversal_key(&sub_steps),
                                ));
                            } else {
                                ctx.select_by_mods.push(SelectByModulator::Key(key.clone()));
                            }
                        } else {
                            ctx.select_by_mods.push(SelectByModulator::Key(key.clone()));
                        }
                    } else if ctx.pending_path {
                        // by() after path() — collect by-key for path element transformation
                        ctx.path_by_keys.push(key.to_string());
                    } else if ctx.pending_dedup {
                        if ctx.dedup_by_consumed {
                            return Err(CompileError::InvalidSequence(
                                "Dedup step can only have one by modulator".into(),
                            ));
                        }
                        // by() after dedup() — dedup by specific property
                        if !key.is_empty() {
                            if let Some(Operator::Distinct { ref mut by_key, .. }) = ctx.current {
                                if let Some(ref existing) = *by_key {
                                    if existing.contains('\x1f') {
                                        // Multi-key dedup: apply property to each label
                                        let new_key = existing
                                            .split('\x1f')
                                            .map(|l| format!("__dedup_prop__{}__{}", key, l))
                                            .collect::<Vec<_>>()
                                            .join("\x1f");
                                        *by_key = Some(new_key);
                                    } else {
                                        *by_key =
                                            Some(format!("__dedup_prop__{}__{}", key, existing));
                                    }
                                } else {
                                    *by_key =
                                        Some(format!("__dedup_prop__{}__{}", key, ctx.current_alias.0));
                                }
                            }
                        }
                        ctx.dedup_by_consumed = true;
                    } else if ctx.pending_aggregate {
                        // by() after aggregate() — project a property from each traverser
                        if let Some(Operator::GremlinAggregate {
                            ref mut by_keys, ..
                        }) = ctx.current
                        {
                            if !key.is_empty() {
                                by_keys.push(key.to_string());
                            }
                        }
                        ctx.pending_aggregate = false;
                        ctx.aggregate_by_consumed = true;
                    } else if ctx.pending_format {
                        // by(key) after format() — compile by-key to a property access
                        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                        let by_alias = self.next_alias("fby");
                        let by_expr = if key.is_empty() {
                            Expr::Variable(ctx.format_pre_alias.clone())
                        } else {
                            make_by_expr(key, &ctx.format_pre_alias)
                        };
                        let mut items = vec![(by_expr, Some(by_alias.clone()))];
                        items.extend(self.label_carry_items());
                        ctx.current = Some(Operator::Project {
                            items,
                            child: Box::new(child),
                        });
                        ctx.format_by_aliases.push(by_alias);
                    } else if ctx.pending_sack_op {
                        // by(key) after sack(op) — rewrite the sack Project to
                        // use a property/label expression instead of ctx.current_alias.
                        if !key.is_empty() {
                            let by_expr = make_by_expr(key, &ctx.current_alias);
                            if let Some(Operator::Project { ref mut items, .. }) = ctx.current {
                                // First item is the sack assignment; rewrite the rhs
                                // (ctx.current_alias reference) with the by expression.
                                if let Some((expr, _)) = items.first_mut() {
                                    rewrite_sack_by(expr, &ctx.current_alias, &by_expr);
                                }
                            }
                            // Filter out traversers where the by-key property is null
                            // (Gremlin drops traversers when by() can't resolve a value).
                            let sack_var = Identifier::new("__sack");
                            if let Some(proj) = ctx.current.take() {
                                ctx.current = Some(Operator::Filter {
                                    predicate: Expr::IsNotNull(Box::new(Expr::Variable(sack_var))),
                                    child: Box::new(proj),
                                });
                            }
                        }
                        ctx.pending_sack_op = false;
                        ctx.pending_sack_by_consumed = true;
                    } else if ctx.pending_project {
                        // by() after project() — replace the default property
                        // expression for the next project key with the by() expression.
                        if ctx.project_by_count < ctx.project_keys.len() {
                            // Check if the key looks like a sub-traversal
                            let looks_like_traversal = key.contains('(') && !key.is_empty();
                            if looks_like_traversal {
                                // Parse and compile the sub-traversal
                                let no_param = |_: &str| -> Option<GremlinValue> { None };
                                if let Ok(sub_steps) =
                                    crate::text_parser::parse_inner_traversal(key, &no_param)
                                {
                                    if !sub_steps.is_empty() {
                                        let mut sub = self.sub_compiler();
                                        if let Ok(sub_op) =
                                            sub.compile_seeded(&sub_steps, &ctx.pre_project_alias)
                                        {
                                            let sub_out_alias = sub.last_alias.clone();
                                            self.absorb(&sub);

                                            // Convert to GremlinProject
                                            match &ctx.current {
                                                Some(Operator::Project { items, child }) => {
                                                    let mut entries: Vec<(
                                                        String,
                                                        Operator,
                                                        Identifier,
                                                        bool,
                                                    )> = Vec::new();
                                                    for (i, (expr, alias_opt)) in
                                                        items.iter().enumerate()
                                                    {
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
                                                                items: vec![(
                                                                    expr.clone(),
                                                                    alias_opt.clone(),
                                                                )],
                                                                child: Box::new(
                                                                    Operator::SingleRow,
                                                                ),
                                                            };
                                                            // Prior entries may have been set by
                                                            // empty by() (identity). We can't
                                                            // recover that info here since it was
                                                            // stored in items[i].0 type. Assume
                                                            // non-identity for safety.
                                                            entries.push((
                                                                ctx.project_keys[i].clone(),
                                                                prop_op,
                                                                alias_opt
                                                                    .as_ref()
                                                                    .cloned()
                                                                    .unwrap_or_else(|| {
                                                                        Identifier::new(
                                                                            &ctx.project_keys[i],
                                                                        )
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
                                            // Skip the default handling below
                                        } else {
                                            // Compilation failed — use simple property expression.
                                            // This is correct for by("propName") but may lose
                                            // semantics for complex by(traversal) modulations.
                                            let by_expr = make_by_expr(key, &ctx.pre_project_alias);
                                            if let Some(Operator::Project {
                                                ref mut items, ..
                                            }) = ctx.current
                                            {
                                                if let Some((expr, _)) =
                                                    items.get_mut(ctx.project_by_count)
                                                {
                                                    *expr = by_expr;
                                                }
                                            }
                                            ctx.project_by_count += 1;
                                            if ctx.project_by_count >= ctx.project_keys.len() {
                                                ctx.pending_project = false;
                                            }
                                        }
                                    } else {
                                        // Empty by() steps — use simple property expression.
                                        let by_expr = make_by_expr(key, &ctx.pre_project_alias);
                                        if let Some(Operator::Project { ref mut items, .. }) =
                                            ctx.current
                                        {
                                            if let Some((expr, _)) = items.get_mut(ctx.project_by_count)
                                            {
                                                *expr = by_expr;
                                            }
                                        }
                                        ctx.project_by_count += 1;
                                        if ctx.project_by_count >= ctx.project_keys.len() {
                                            ctx.pending_project = false;
                                        }
                                    }
                                } else {
                                    // Parse failed — use simple property expression.
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
                                    ref mut entries,
                                    ..
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
                                                Some(Identifier::new(
                                                    &ctx.project_keys[ctx.project_by_count],
                                                )),
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
                    } else if !ctx.pending_where_by_targets.is_empty() {
                        // by() after where() records a traversal-ring modulator
                        // and applies it only once the full by-list is known.
                        ctx.pending_where_by_modulators.push(key.to_string());
                    } else if ctx.aggregate_by_consumed {
                        // Second by() after aggregate — error per Gremlin spec
                        return Err(CompileError::InvalidSequence(
                            "Aggregate step can only have one by modulator".into(),
                        ));
                    } else if ctx.pending_sample && ctx.sample_by_consumed {
                        return Err(CompileError::InvalidSequence(
                            "Sample step can only have one by modulator".into(),
                        ));
                    } else if ctx.pending_sack_by_consumed {
                        return Err(CompileError::InvalidSequence(
                            "Sack step can only have one by modulator".into(),
                        ));
                    } else if ctx.pending_valuemap && ctx.valuemap_by_consumed {
                        return Err(CompileError::InvalidSequence(
                            "step can only have one by modulator".into(),
                        ));
                    }
                    // Track by() consumption for sample/sack/valueMap
                    if ctx.pending_sample && !ctx.sample_by_consumed {
                        ctx.sample_by_consumed = true;
                        ctx.pending_sample_weight = Some(if key.is_empty() {
                            Expr::Variable(ctx.current_alias.clone())
                        } else {
                            make_by_expr(key, &ctx.current_alias)
                        });
                    }
                    if ctx.pending_sack_op && !ctx.pending_sack_by_consumed {
                        ctx.pending_sack_by_consumed = true;
                    }
                    if ctx.pending_valuemap && !ctx.valuemap_by_consumed {
                        ctx.valuemap_by_consumed = true;
                    }
                }

                GremlinStep::ByTraversal(sub_steps, _descending) => {
                    // by(traversal) after valueMap — unfold list values
                    if ctx.pending_valuemap && !ctx.valuemap_by_consumed {
                        let is_unfold =
                            sub_steps.len() == 1 && matches!(sub_steps[0], GremlinStep::Unfold);
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
                        ctx.valuemap_by_consumed = true;
                    } else if ctx.pending_valuemap && ctx.valuemap_by_consumed {
                        return Err(CompileError::InvalidSequence(
                            "step can only have one by modulator".into(),
                        ));
                    } else if ctx.pending_project {
                        // by(traversal) after project() — compile the sub-traversal
                        // and convert to GremlinProject if not already done.
                        if ctx.project_by_count < ctx.project_keys.len() {
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
                                    let mut entries: Vec<(String, Operator, Identifier, bool)> =
                                        Vec::new();
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
                                                let is_ident =
                                                    matches!(&items[i].0, Expr::Variable(_));
                                                let prop_op = Operator::Project {
                                                    items: vec![(
                                                        items[i].0.clone(),
                                                        alias_opt.clone(),
                                                    )],
                                                    child: Box::new(Operator::SingleRow),
                                                };
                                                entries.push((
                                                    ctx.project_keys[i].clone(),
                                                    prop_op,
                                                    alias_opt.as_ref().cloned().unwrap_or_else(
                                                        || Identifier::new(&ctx.project_keys[i]),
                                                    ),
                                                    is_ident,
                                                ));
                                            } else {
                                                // Not yet filled — default to property access
                                                let prop_op = Operator::Project {
                                                    items: vec![(
                                                        items[i].0.clone(),
                                                        alias_opt.clone(),
                                                    )],
                                                    child: Box::new(Operator::SingleRow),
                                                };
                                                entries.push((
                                                    ctx.project_keys[i].clone(),
                                                    prop_op,
                                                    alias_opt.as_ref().cloned().unwrap_or_else(
                                                        || Identifier::new(&ctx.project_keys[i]),
                                                    ),
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
                        }
                    }
                    // Non-project contexts: delegate ByTraversal to the same
                    // behavior as By by reconstructing the original key string.
                    if !ctx.pending_project {
                        if ctx.pending_order {
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
                                items: vec![(Expr::Variable(sub_out_alias), !_descending)],
                                gremlin_order: true,
                                child: Box::new(applied),
                            });
                            ctx.pending_order = false;
                        } else if ctx.pending_select {
                            // by(traversal) after select() — preserve the
                            // real traversal for per-key execution.
                            ctx.select_by_mods.push(SelectByModulator::Traversal(sub_steps.to_vec()));
                        } else if ctx.pending_path {
                            // by(traversal) after path() — extract property key
                            // from patterns like by(values('name').toUpper())
                            let key = extract_by_traversal_key(sub_steps);
                            ctx.path_by_keys.push(key);
                        } else if ctx.pending_dedup {
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
                        } else if !ctx.pending_where_by_targets.is_empty() {
                            ctx.pending_where_by_modulators.push(extract_by_traversal_key(sub_steps));
                        } else if ctx.pending_group {
                            // by(traversal) after group() — compile sub-traversal for key or value
                            if ctx.pending_group_by_count == 0 {
                                // First by(traversal) = group key — traversal is not identity
                                if let Some(Operator::GremlinGroup {
                                    ref mut key_is_identity,
                                    ..
                                }) = ctx.current
                                {
                                    *key_is_identity = false;
                                }
                                let mut sub = self.sub_compiler();
                                let sub_op = sub.compile_seeded(sub_steps, &ctx.pre_group_alias)?;
                                let sub_out_alias = sub.last_alias.clone();
                                self.absorb(&sub);
                                if let Some(Operator::GremlinGroup {
                                    ref mut key_traversal,
                                    ..
                                }) = ctx.current
                                {
                                    *key_traversal = Some((Box::new(sub_op), sub_out_alias));
                                }
                                ctx.pending_group_by_count = 1;
                            } else {
                                // Second by(traversal) = value aggregation
                                // Check for trailing agg step (count/sum/fold)
                                let (proj_steps, trailing_agg) =
                                    split_trailing_group_value_agg(sub_steps);

                                if proj_steps.is_empty() {
                                    if let Some(Operator::GremlinGroup {
                                        ref mut agg_fn,
                                        ref mut explicit_fold,
                                        ..
                                    }) = ctx.current
                                    {
                                        if let Some(agg) = trailing_agg {
                                            if matches!(agg, AggFunction::Collect) {
                                                *explicit_fold = true;
                                            }
                                            *agg_fn = agg;
                                        }
                                    }
                                } else {
                                    let mut sub = self.sub_compiler();
                                    // If the element going into group() is a single-entry Map
                                    // from unfold() (i.e., a Map.Entry), mark the sub-compiler
                                    // so that select(Column.keys/values) uses map-entry semantics
                                    // (returning the key/value directly, not a collection).
                                    let outer_child_is_unwind = if let Some(
                                        Operator::GremlinGroup { child, .. },
                                    ) = &ctx.current
                                    {
                                        matches!(child.as_ref(), Operator::Unwind { .. })
                                    } else {
                                        false
                                    };
                                    sub.seed_is_map_entry = outer_child_is_unwind;
                                    let sub_op =
                                        sub.compile_seeded(proj_steps, &ctx.pre_group_alias)?;
                                    let sub_out_alias = sub.last_alias.clone();
                                    self.absorb(&sub);
                                    if let Some(Operator::GremlinGroup {
                                        ref mut value_traversal,
                                        ref mut agg_fn,
                                        ref mut explicit_fold,
                                        ..
                                    }) = ctx.current
                                    {
                                        *value_traversal = Some((Box::new(sub_op), sub_out_alias));
                                        if let Some(agg) = trailing_agg {
                                            if matches!(agg, AggFunction::Collect) {
                                                *explicit_fold = true;
                                            }
                                            *agg_fn = agg;
                                        }
                                    }
                                }
                                ctx.pending_group = false;
                            }
                        } else if ctx.pending_aggregate {
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
                        } else if ctx.pending_format {
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
                        } else if ctx.pending_sack_op && !ctx.pending_sack_by_consumed {
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
                        }
                    }
                }

                GremlinStep::Limit(n) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    ctx.current = Some(Operator::Limit {
                        count: Expr::Integer(*n),
                        child: Box::new(child),
                    });
                }

                GremlinStep::Range(low, high) => {
                    if *high < *low {
                        return Err(CompileError::InvalidSequence(format!(
                            "range({}, {}): high must be >= low",
                            low, high
                        )));
                    }
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let skipped = if *low > 0 {
                        Operator::Skip {
                            count: Expr::Integer(*low),
                            child: Box::new(child),
                        }
                    } else {
                        child
                    };
                    ctx.current = Some(Operator::Limit {
                        count: Expr::Integer(*high - *low),
                        child: Box::new(skipped),
                    });
                }

                GremlinStep::LimitLocal(n) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    ctx.current = Some(Operator::GremlinMapSlice {
                        alias: ctx.current_alias.clone(),
                        lo: 0,
                        hi: *n as usize,
                        child: Box::new(child),
                    });
                }

                GremlinStep::RangeLocal(low, high) => {
                    // Flush pending order(Scope.local) so the sort is applied
                    // before slicing.
                    if ctx.pending_order && ctx.pending_order_local {
                        if let Some(child) = ctx.current.take() {
                            let alias = self.next_alias("srtl");
                            let mut items = vec![(
                                Expr::FunctionCall {
                                    name: SmolStr::new("gremlin_sort_local"),
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
                        ctx.pending_order = false;
                        ctx.pending_order_local = false;
                    }
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    ctx.current = Some(Operator::GremlinMapSlice {
                        alias: ctx.current_alias.clone(),
                        lo: *low as usize,
                        hi: *high as usize,
                        child: Box::new(child),
                    });
                }

                GremlinStep::SkipLocal(n) => {
                    // Flush pending order(Scope.local) so the sort is applied
                    // before skipping.
                    if ctx.pending_order && ctx.pending_order_local {
                        if let Some(child) = ctx.current.take() {
                            let alias = self.next_alias("srtl");
                            let mut items = vec![(
                                Expr::FunctionCall {
                                    name: SmolStr::new("gremlin_sort_local"),
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
                        ctx.pending_order = false;
                        ctx.pending_order_local = false;
                    }
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    ctx.current = Some(Operator::GremlinMapSlice {
                        alias: ctx.current_alias.clone(),
                        lo: *n as usize,
                        hi: usize::MAX,
                        child: Box::new(child),
                    });
                }

                GremlinStep::SortLocal => {
                    // Sort elements within a list/map value.
                    // Defer to by() handler; if no by() follows, resolve at end.
                    ctx.pending_order = true;
                    ctx.pending_order_local = true;
                }

                GremlinStep::Dedup(keys) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    // dedup() — deduplicate by the ctx.current traverser value
                    // dedup("a","b") — deduplicate by values at step labels
                    let by_key = if keys.is_empty() {
                        Some(ctx.current_alias.to_string())
                    } else if keys.len() == 1 {
                        // Resolve label to internal alias
                        let resolved = self
                            .alias_map
                            .get(&keys[0])
                            .map(|a| a.to_string())
                            .unwrap_or_else(|| keys[0].clone());
                        Some(resolved)
                    } else {
                        // Multi-key dedup: resolve each label to its alias
                        let resolved: Vec<String> = keys
                            .iter()
                            .map(|k| {
                                self.alias_map
                                    .get(k)
                                    .map(|a| a.to_string())
                                    .unwrap_or_else(|| k.clone())
                            })
                            .collect();
                        Some(resolved.join("\x1f"))
                    };
                    ctx.current = Some(Operator::Distinct {
                        child: Box::new(child),
                        by_key,
                        productive: self.productive_by,
                    });
                    ctx.pending_dedup = true;
                }

                GremlinStep::Where(inner_steps) | GremlinStep::Not(inner_steps) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let is_not = matches!(step, GremlinStep::Not(_));

                    // Use predicate mode only when ALL inner steps are simple
                    // predicate-convertible steps. Otherwise use traversal mode
                    // (sub-traversal existence check via Apply + Count + Filter).
                    // When there are multiple Select steps, they need chaining
                    // (output of one feeds into the next), so use sub-traversal
                    // mode instead of predicate mode.
                    let select_count = inner_steps
                        .iter()
                        .filter(|s| matches!(s, GremlinStep::Select(..)))
                        .count();
                    let can_be_predicate = !inner_steps.is_empty()
                        && select_count <= 1
                        && inner_steps.iter().all(|s| {
                            matches!(
                                s,
                                GremlinStep::Is(_)
                                    | GremlinStep::Has(_, _)
                                    | GremlinStep::HasPredicate(_, _)
                                    | GremlinStep::HasLabel(_)
                                    | GremlinStep::Values(_)
                                    | GremlinStep::Label
                                    | GremlinStep::Id
                                    | GremlinStep::Select(..)
                            )
                        });

                    if !can_be_predicate {
                        // Compile inner steps as a sub-traversal seeded with
                        // the ctx.current alias. Use Apply + Count + Filter to
                        // implement existence semantics.
                        // If the sub-traversal starts with as("label"), seed
                        // from that label's existing alias instead.
                        let mut sub = self.sub_compiler();
                        // Propagate map-entry context for unfolded traversals
                        if matches!(&child, Operator::Unwind { .. }) || self.seed_is_map_entry {
                            sub.seed_is_map_entry = true;
                        }
                        let (seed_alias, steps_slice) = match inner_steps.first() {
                            Some(GremlinStep::As(label)) if self.alias_map.contains_key(label) => {
                                (self.alias_map[label].clone(), &inner_steps[1..])
                            }
                            _ => (ctx.current_alias.clone(), &inner_steps[..]),
                        };

                        // Check if the sub-traversal ends with as("label")
                        // referencing an already-bound variable.  If so, the
                        // endpoint must be constrained to match that binding.
                        let endpoint_target =
                            if let Some(GremlinStep::As(label)) = steps_slice.last() {
                                self.alias_map.get(label).cloned()
                            } else {
                                None
                            };

                        let inner_op = sub.compile_seeded(steps_slice, &seed_alias)?;
                        // For not()/where() sub-traversals, only advance the
                        // alias counter (to avoid conflicts with outer scope).
                        // Do NOT merge alias_map — internal aliases from inside
                        // not()/where() must not pollute the outer scope, or
                        // downstream Project operators will attempt to reference
                        // those internal aliases on rows that don't carry them.
                        self.alias_counter = sub.alias_counter;
                        let sub_result_alias = sub.last_alias.clone();

                        // If the endpoint is constrained, add a filter so only
                        // traversers that land on the bound variable pass.
                        let inner_op = if let Some(target_alias) = endpoint_target {
                            Operator::Filter {
                                predicate: Expr::BinaryOp {
                                    op: BinaryOp::Eq,
                                    left: Box::new(Expr::Variable(sub_result_alias.clone())),
                                    right: Box::new(Expr::Variable(target_alias)),
                                },
                                child: Box::new(inner_op),
                            }
                        } else {
                            inner_op
                        };

                        // Count the sub-traversal results
                        let cnt_alias = self.next_alias("wcnt");
                        let count_op = Operator::Aggregate {
                            keys: vec![],
                            aggregations: vec![(
                                cypher_core::planner::AggFunction::Count,
                                Expr::Variable(sub_result_alias),
                                None,
                                false,
                                cnt_alias.clone(),
                            )],
                            child: Box::new(inner_op),
                        };

                        // Wrap in Apply so each input row seeds the sub-traversal
                        let apply = Operator::Apply {
                            outer: Box::new(child),
                            inner: Box::new(count_op),
                        };

                        // Filter: keep rows where count > 0 (or == 0 for Not)
                        let predicate = if is_not {
                            Expr::BinaryOp {
                                op: BinaryOp::Eq,
                                left: Box::new(Expr::Variable(cnt_alias)),
                                right: Box::new(Expr::Integer(0)),
                            }
                        } else {
                            Expr::BinaryOp {
                                op: BinaryOp::Gt,
                                left: Box::new(Expr::Variable(cnt_alias)),
                                right: Box::new(Expr::Integer(0)),
                            }
                        };
                        ctx.current = Some(Operator::Filter {
                            predicate,
                            child: Box::new(apply),
                        });
                    } else {
                        let inner_pred = steps_to_predicate(
                            inner_steps,
                            &ctx.current_alias,
                            &self.alias_map,
                            &self.aggregate_keys,
                        );
                        let predicate = if is_not {
                            // Wrap in COALESCE so that Null (from cross-type
                            // comparison) becomes false before negation. This
                            // ensures not(has("name", P.gt(27))) correctly
                            // keeps traversers when the comparison is undefined.
                            let coalesced = Expr::FunctionCall {
                                name: SmolStr::new("coalesce"),
                                args: vec![inner_pred, Expr::Bool(false)],
                                distinct: false,
                            };
                            Expr::UnaryOp {
                                op: cypher_core::ast::UnaryOp::Not,
                                expr: Box::new(coalesced),
                            }
                        } else {
                            inner_pred
                        };
                        // Track traversal-ring targets for by() modulation on where().
                        if !is_not {
                            if let Some(targets) = extract_where_by_targets(
                                inner_steps,
                                &self.alias_map,
                                &ctx.current_alias,
                            ) {
                                ctx.pending_where_by_targets = targets;
                                ctx.pending_where_by_modulators.clear();
                            }
                        }
                        ctx.current = Some(Operator::Filter {
                            predicate,
                            child: Box::new(child),
                        });
                    }
                }

                GremlinStep::And(branches) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    if branches.is_empty() {
                        ctx.current = Some(child);
                    } else {
                        ctx.current = Some(self.compile_connective_branches(
                            branches,
                            &ctx.current_alias,
                            child,
                            BinaryOp::And,
                        )?);
                    }
                }

                GremlinStep::Or(branches) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    if branches.is_empty() {
                        ctx.current = Some(child);
                    } else {
                        ctx.current = Some(self.compile_connective_branches(
                            branches,
                            &ctx.current_alias,
                            child,
                            BinaryOp::Or,
                        )?);
                    }
                }

                GremlinStep::Is(predicate) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let pred_expr =
                        if let Predicate::IsType(GremlinValue::String(expected)) = predicate {
                            if let Some(ref subtype) = self.numeric_subtype {
                                if subtype == expected {
                                    Expr::Bool(true)
                                } else {
                                    Expr::Bool(false)
                                }
                            } else {
                                predicate_to_expr(predicate, &ctx.current_alias)
                            }
                        } else if let Some(label_expr) =
                            predicate_label_ref(predicate, &self.alias_map, &self.aggregate_keys)
                        {
                            let op = predicate_to_binary_op(predicate);
                            Expr::BinaryOp {
                                op,
                                left: Box::new(Expr::Variable(ctx.current_alias.clone())),
                                right: Box::new(label_expr),
                            }
                        } else if predicate_has_label_refs(
                            predicate,
                            &self.alias_map,
                            &self.aggregate_keys,
                        ) {
                            predicate_to_label_aware_expr(
                                predicate,
                                &Expr::Variable(ctx.current_alias.clone()),
                                &self.alias_map,
                                &self.aggregate_keys,
                            )
                        } else {
                            predicate_to_expr(predicate, &ctx.current_alias)
                        };
                    ctx.current = Some(Operator::Filter {
                        predicate: pred_expr,
                        child: Box::new(child),
                    });
                }

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
                        child: ctx.current.map(Box::new),
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
                        child: ctx.current.map(Box::new),
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
                        child: ctx.current.map(Box::new),
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
                        child: Box::new(ctx.current.unwrap_or(Operator::SingleRow)),
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
                    ctx.pending_properties.push((resolved_key.clone(), value.clone()));
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
                                continue;
                            }
                        }
                        // Bare string label reference: from("a")
                        if let GremlinStep::Constant(GremlinValue::String(label)) = &inner_steps[0]
                        {
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
                            continue;
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

                GremlinStep::Path => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    ctx.current = Some(Operator::GremlinPath {
                        child: Box::new(child),
                        path_aliases: ctx.path_aliases.clone(),
                        by_keys: vec![],
                        productive: self.productive_by,
                        from_label: None,
                        to_label: None,
                    });
                    ctx.current_alias = Identifier::new("path");
                    ctx.pending_path = true;
                    ctx.path_by_keys.clear();
                    // Path has consumed the aliases — stop carrying them
                    // through subsequent Projects.
                    self.path_tracking.clear();
                }

                GremlinStep::Repeat(label, body_steps) => {
                    // Validate: inject() is not allowed inside repeat()
                    if body_steps
                        .iter()
                        .any(|s| matches!(s, GremlinStep::Inject(_)))
                    {
                        return Err(CompileError::InvalidSequence(
                            "The parent of inject()-step can not be repeat()-step".into(),
                        ));
                    }
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let seed_alias = ctx.current_alias.clone();

                    // Pre-scan: when Pop-select is downstream, check if the
                    // body re-binds any existing label.  Create shadow aliases
                    // so the executor can preserve original bindings for
                    // Pop.first.
                    let mut pop_origins: Vec<(String, String)> = Vec::new();
                    if self.needs_label_stacks {
                        for step in body_steps.iter() {
                            if let GremlinStep::As(lbl) = step {
                                if let Some(orig) = self.alias_map.get(lbl).cloned() {
                                    let shadow = self.next_alias("pop_orig");
                                    self.label_stacks
                                        .entry(lbl.clone())
                                        .or_default()
                                        .insert(0, shadow.clone());
                                    pop_origins.push((
                                        shadow.as_str().to_string(),
                                        orig.as_str().to_string(),
                                    ));
                                }
                            }
                        }
                    }

                    let mut sub = self.sub_compiler();
                    if self.seed_is_map_entry {
                        sub.seed_is_map_entry = true;
                    }
                    let body = sub.compile_seeded(body_steps, &ctx.current_alias)?;
                    let body_out_alias = sub.last_alias.clone();
                    // Collect intermediate body path aliases (between seed and
                    // body-output) so the executor can accumulate edge values
                    // in __gremlin_path_acc for patterns like outE().inV().
                    let body_pa: Vec<String> = if self.needs_path_tracking {
                        sub.path_tracking
                            .iter()
                            .filter(|id| {
                                id.as_str() != seed_alias.as_str()
                                    && id.as_str() != body_out_alias.as_str()
                            })
                            .map(|id| id.as_str().to_string())
                            .collect()
                    } else {
                        vec![]
                    };
                    self.absorb(&sub);
                    // Apply any pending emit/until that appeared before the repeat
                    let emit_val = ctx.pending_emit;
                    ctx.pending_emit = false;
                    let until_val = if let Some(until_steps) = ctx.pending_until.take() {
                        let mut sub2 = self.sub_compiler();
                        let until_op = sub2.compile_seeded(&until_steps, &ctx.current_alias)?;
                        self.absorb(&sub2);
                        Some(Box::new(until_op))
                    } else {
                        None
                    };
                    // If body produces a different alias, remap it back to the seed
                    let remap = if body_out_alias.as_str() != seed_alias.as_str() {
                        Some((
                            body_out_alias.as_str().to_string(),
                            seed_alias.as_str().to_string(),
                        ))
                    } else {
                        None
                    };
                    let times_val = ctx.pending_times.take();
                    let times_before_val = times_val.is_some();
                    ctx.current = Some(Operator::Repeat {
                        body: Box::new(body),
                        times: times_val,
                        until: until_val,
                        emit: emit_val,
                        emit_before: emit_val,
                        emit_predicate: if let Some(ep_steps) = ctx.pending_emit_pred.take() {
                            let mut sub3 = self.sub_compiler();
                            let ep_op = sub3.compile_seeded(&ep_steps, &ctx.current_alias)?;
                            self.absorb(&sub3);
                            Some(Box::new(ep_op))
                        } else {
                            None
                        },
                        remap,
                        times_before: times_before_val,
                        loop_label: label.as_deref().map(Box::from),
                        body_path_aliases: body_pa,
                        pop_origins,
                        needs_path: self.needs_path_tracking,
                        child: Box::new(child),
                    });
                }

                GremlinStep::Times(n) => {
                    // Modify preceding Repeat
                    if let Some(Operator::Repeat {
                        ref mut times,
                        ref mut times_before,
                        ..
                    }) = ctx.current
                    {
                        *times = Some(*n as usize);
                        *times_before = false; // times() after repeat() → do-while
                    } else {
                        // times() before repeat() — store as pending
                        ctx.pending_times = Some(*n as usize);
                    }
                }

                GremlinStep::Until(until_steps) => {
                    if let Some(Operator::Repeat { ref mut until, .. }) = ctx.current {
                        // Modify preceding Repeat
                        let mut sub = self.sub_compiler();
                        let until_op = sub.compile_seeded(until_steps, &ctx.current_alias)?;
                        self.absorb(&sub);
                        *until = Some(Box::new(until_op));
                    } else {
                        // until() before repeat() — store as pending
                        ctx.pending_until = Some(until_steps.clone());
                    }
                }

                GremlinStep::Emit => {
                    if let Some(Operator::Repeat {
                        ref mut emit,
                        ref mut emit_before,
                        ..
                    }) = ctx.current
                    {
                        *emit = true;
                        // emit() after repeat() → post-emit (don't include starting traversers)
                        *emit_before = false;
                    } else {
                        // emit() before repeat() — store as pending
                        ctx.pending_emit = true;
                    }
                }

                GremlinStep::EmitPred(pred_steps) => {
                    if let Some(Operator::Repeat {
                        ref mut emit,
                        ref mut emit_before,
                        ref mut emit_predicate,
                        ..
                    }) = ctx.current
                    {
                        *emit = true;
                        *emit_before = false;
                        let mut sub = self.sub_compiler();
                        let ep_op = sub.compile_seeded(pred_steps, &ctx.current_alias)?;
                        self.absorb(&sub);
                        *emit_predicate = Some(Box::new(ep_op));
                    } else {
                        // emit(pred) before repeat() — store as pending
                        ctx.pending_emit = true;
                        ctx.pending_emit_pred = Some(pred_steps.clone());
                    }
                }

                GremlinStep::Choose(pred_steps, true_steps, false_steps) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let mut sub = self.sub_compiler();
                    if true_steps.is_empty() && false_steps.is_none() {
                        // 1-arg choose: value-based dispatch, options come from Option steps
                        let selector = sub.compile_seeded(pred_steps, &ctx.current_alias)?;
                        let sel_alias = sub.last_alias.clone();
                        self.absorb(&sub);
                        let choose_alias = self.next_alias("chm");
                        ctx.current = Some(Operator::ChooseMap {
                            selector: Box::new(selector),
                            selector_alias: sel_alias,
                            options: Vec::new(),
                            child: Box::new(child),
                            output_alias: Some(choose_alias.clone()),
                        });
                        // Defer alias switch until after all option() steps
                        ctx.pending_choose_output = Some(choose_alias);
                    } else {
                        let pred_op = sub.compile_seeded(pred_steps, &ctx.current_alias)?;
                        let true_op = sub.compile_seeded(true_steps, &ctx.current_alias)?;
                        let t_alias = sub.last_alias.clone();
                        let false_op = match false_steps {
                            Some(steps) => {
                                Some(Box::new(sub.compile_seeded(steps, &ctx.current_alias)?))
                            }
                            None => None,
                        };
                        // For false branch: use the sub-compiler's alias if a false
                        // branch exists, otherwise use the seed alias (ctx.current_alias
                        // before choose) for pass-through rows.
                        let f_alias = if false_steps.is_some() {
                            sub.last_alias.clone()
                        } else {
                            ctx.current_alias.clone()
                        };
                        self.absorb(&sub);

                        // Create a unified output alias for choose branches.
                        let choose_alias = self.next_alias("ch");
                        ctx.current_alias = choose_alias.clone();

                        ctx.current = Some(Operator::Choose {
                            predicate: Box::new(pred_op),
                            if_true: Box::new(true_op),
                            if_false: false_op,
                            child: Box::new(child),
                            true_alias: Some(t_alias),
                            false_alias: Some(f_alias),
                            output_alias: Some(choose_alias.clone()),
                        });
                    }
                }

                GremlinStep::Coalesce(alternatives) => {
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
                }

                GremlinStep::Union(branches) => {
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
                                    let mut items = vec![(
                                        Expr::Variable(src_alias.clone()),
                                        Some(union_alias.clone()),
                                    )];
                                    // Carry forward all step labels
                                    items.extend(self.label_carry_items());

                                    // When path tracking is active, build
                                    // __gremlin_path_acc from the branch's
                                    // intermediate path aliases (all but the
                                    // last, which becomes the union output).
                                    if self.needs_path_tracking && branch_paths.len() > 1 {
                                        let acc_items: Vec<Expr> = branch_paths
                                            [..branch_paths.len() - 1]
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
                }

                GremlinStep::SideEffect(effect_steps) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let mut sub = self.sub_compiler();
                    let effect = sub.compile_seeded(effect_steps, &ctx.current_alias)?;
                    self.absorb(&sub);
                    ctx.current = Some(Operator::GremlinSideEffect {
                        effect: Box::new(effect),
                        child: Box::new(child),
                    });
                }

                GremlinStep::Aggregate(key) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let is_set = self.set_keys.contains(key);
                    let is_assign = self
                        .reductions
                        .get(key)
                        .is_some_and(|(op, _)| op == "assign");
                    ctx.current = Some(Operator::GremlinAggregate {
                        key: key.clone(),
                        child: Box::new(child),
                        value_alias: ctx.current_alias.clone(),
                        by_keys: vec![],
                        by_traversal: None,
                        productive: self.productive_by,
                        set_semantics: is_set,
                        assign_semantics: is_assign,
                    });
                    // Track aggregate keys separately for within/without
                    // resolution (not in alias_map to avoid conflicts).
                    self.aggregate_keys.insert(key.clone());
                    ctx.pending_aggregate = true;
                }

                GremlinStep::Cap(keys) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    // Collect reduction metadata from compiler state
                    let mut reductions: Vec<(String, String, f64)> = Vec::new();
                    for k in keys {
                        if let Some((op, init)) = self.reductions.get(k) {
                            reductions.push((k.clone(), op.clone(), *init));
                        }
                    }
                    ctx.current = Some(Operator::Cap {
                        keys: keys.clone(),
                        child: Box::new(child),
                        reductions,
                    });
                    // After cap(), the result row uses cap key names as columns.
                    // Update ctx.current_alias so subsequent steps reference the right column.
                    if keys.len() == 1 {
                        ctx.current_alias = Identifier::new(keys[0].clone());
                    } else {
                        // Multi-key cap: the executor creates a combined Map
                        // under a joined alias so unfold() can iterate entries.
                        ctx.current_alias = Identifier::new(keys.join("_"));
                    }
                }

                GremlinStep::Constant(val) => {
                    let child = ctx.current.take().unwrap_or(Operator::SingleRow);
                    let alias = self.next_alias("const");
                    let mut items = vec![(gremlin_value_to_expr(val), Some(alias.clone()))];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    ctx.current_alias = alias;
                }

                GremlinStep::Inject(values) => {
                    if values.is_empty() {
                        // inject() with no args produces an empty stream
                        if ctx.current.is_none() {
                            ctx.current = Some(Operator::Filter {
                                predicate: Expr::Bool(false),
                                child: Box::new(Operator::SingleRow),
                            });
                        }
                    } else {
                        let exprs: Vec<Expr> = values.iter().map(gremlin_value_to_expr).collect();
                        let alias = self.next_alias("inj");
                        let injected = Operator::Unwind {
                            list_expr: Expr::List(exprs),
                            alias: alias.clone(),
                            child: Box::new(Operator::SingleRow),
                        };
                        if let Some(child) = ctx.current.take() {
                            // Check if we have a real data source or just
                            // configuration steps (withSideEffect, strategies).
                            if ctx.current_alias.0.as_str() == "__gremlin_v" {
                                // Source inject with configuration context:
                                // no real traversers to union with.
                                // Produce injected rows with side-effect
                                // columns projected as constants.
                                let union_alias = self.next_alias("inj");
                                let mut items =
                                    vec![(Expr::Variable(alias), Some(union_alias.clone()))];
                                for (se_key, se_ident) in &self.alias_map {
                                    if se_ident.0.starts_with("__side_effect_") {
                                        if let Some(val) = self.side_effect_values.get(se_key) {
                                            items.push((
                                                gremlin_value_to_expr(val),
                                                Some(se_ident.clone()),
                                            ));
                                        }
                                    }
                                }
                                ctx.current = Some(Operator::Project {
                                    items,
                                    child: Box::new(injected),
                                });
                                ctx.current_alias = union_alias;
                            } else {
                                // Mid-traversal: UNION child stream with injected
                                // values, then project to a common alias.
                                let prev_alias = ctx.current_alias.clone();
                                let union_alias = self.next_alias("inj");
                                let mut left_items = vec![(
                                    Expr::Variable(prev_alias.clone()),
                                    Some(union_alias.clone()),
                                )];
                                left_items.extend(self.label_carry_items());
                                let left = Operator::Project {
                                    items: left_items,
                                    child: Box::new(child),
                                };
                                let right = Operator::Project {
                                    items: {
                                        let mut items = vec![(
                                            Expr::Variable(alias),
                                            Some(union_alias.clone()),
                                        )];
                                        // Carry side-effect columns into injected
                                        // rows so select("k") can find them.
                                        for (se_key, se_ident) in &self.alias_map {
                                            if se_ident.0.starts_with("__side_effect_") {
                                                if let Some(val) =
                                                    self.side_effect_values.get(se_key)
                                                {
                                                    items.push((
                                                        gremlin_value_to_expr(val),
                                                        Some(se_ident.clone()),
                                                    ));
                                                }
                                            }
                                        }
                                        items
                                    },
                                    child: Box::new(injected),
                                };
                                ctx.current = Some(Operator::Union {
                                    left: Box::new(left),
                                    right: Box::new(right),
                                    all: true,
                                });
                                ctx.current_alias = union_alias.clone();
                                // Replace prev_alias with union_alias in path
                                // tracking so injected traversers contribute
                                // to path() while original traversers don't
                                // duplicate.
                                if let Some(pos) =
                                    ctx.path_aliases.iter().position(|a| *a == prev_alias)
                                {
                                    ctx.path_aliases[pos] = union_alias.clone();
                                }
                                if let Some(pos) =
                                    self.path_tracking.iter().position(|a| *a == prev_alias)
                                {
                                    self.path_tracking[pos] = union_alias;
                                }
                            }
                        } else {
                            // Source step: just unwind
                            ctx.current = Some(injected);
                            ctx.current_alias = alias.clone();
                            ctx.path_aliases.push(alias);
                            self.path_tracking.push(ctx.current_alias.clone());
                        }
                    }
                }

                GremlinStep::Match(patterns) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let mut compiled_patterns = Vec::with_capacity(patterns.len());
                    let mut all_alias_pairs: Vec<(String, Identifier)> = Vec::new();
                    let mut pattern_labels_list: Vec<Vec<(String, Identifier)>> = Vec::new();
                    // Separate filter patterns (not/where) from join patterns.
                    // Filter patterns are applied as post-filters after the match join.
                    let mut filter_pattern_steps: Vec<Vec<GremlinStep>> = Vec::new();
                    // Track whether the input traverser has been bound to a
                    // label. Only the first new label gets the input; subsequent
                    // new labels enumerate via V(None).
                    let mut input_bound = false;
                    // Flatten and() patterns: match(a, and(b, c)) → match(a, b, c)
                    let mut flat_patterns: Vec<Vec<GremlinStep>> = Vec::new();
                    fn flatten_match_patterns(
                        patterns: &[Vec<GremlinStep>],
                        out: &mut Vec<Vec<GremlinStep>>,
                    ) {
                        for p in patterns {
                            if p.len() == 1 {
                                if let Some(GremlinStep::And(branches)) = p.first() {
                                    flatten_match_patterns(branches, out);
                                    continue;
                                }
                            }
                            out.push(p.clone());
                        }
                    }
                    flatten_match_patterns(patterns, &mut flat_patterns);

                    for pattern_steps in &flat_patterns {
                        // Detect filter-only patterns: a single Not or Where step
                        let is_filter = pattern_steps.len() == 1
                            && matches!(
                                pattern_steps.first(),
                                Some(GremlinStep::Not(_)) | Some(GremlinStep::Where(_))
                            );
                        if is_filter {
                            filter_pattern_steps.push(pattern_steps.clone());
                            continue;
                        }

                        let mut sub = self.sub_compiler();

                        // Check if the pattern starts with As("X").
                        let first_as_label = pattern_steps.first().and_then(|s| {
                            if let GremlinStep::As(label) = s {
                                Some(label.clone())
                            } else {
                                None
                            }
                        });

                        // Track whether the seed label is already bound
                        let seed_already_bound = first_as_label
                            .as_ref()
                            .map(|l| sub.alias_map.contains_key(l))
                            .unwrap_or(false);

                        let compiled = if let Some(ref label) = first_as_label {
                            if let Some(existing_alias) = sub.alias_map.get(label).cloned() {
                                // Label already bound — use existing alias as seed.
                                let remaining = &pattern_steps[1..];
                                sub.compile_with_seed(remaining, existing_alias)?
                            } else if !input_bound {
                                // First new label — bind to the ctx.current traverser.
                                input_bound = true;
                                sub.alias_map.insert(label.clone(), ctx.current_alias.clone());
                                sub.compile_with_seed(&pattern_steps[1..], ctx.current_alias.clone())?
                            } else {
                                // Subsequent new label — create an explicit
                                // NodeScan with a known alias so the label maps
                                // to the correct column.
                                let scan_alias = sub.next_alias("v");
                                sub.alias_map.insert(label.clone(), scan_alias.clone());
                                let scan = Operator::NodeScan {
                                    label: None,
                                    alias: scan_alias.clone(),
                                };
                                sub.compile_from(
                                    &pattern_steps[1..],
                                    Some(scan),
                                    scan_alias.clone(),
                                    vec![scan_alias],
                                )?
                            }
                        } else {
                            // No leading as() — seed from the ctx.current traverser.
                            sub.compile_with_seed(pattern_steps, ctx.current_alias.clone())?
                        };

                        // Determine per-pattern produced labels: all labels
                        // EXCEPT the seed label when it was already bound AND
                        // its alias collides with another label (Repeat remap).
                        let produced: Vec<(String, Identifier)> = sub
                            .alias_map
                            .iter()
                            .filter(|(label, alias)| {
                                if seed_already_bound && first_as_label.as_ref() == Some(label) {
                                    // If the seed alias is shared with another
                                    // label, exclude seed — its value may be
                                    // overwritten by the pattern (e.g. Repeat).
                                    let shared = sub
                                        .alias_map
                                        .iter()
                                        .any(|(l, a)| l != *label && a.as_str() == alias.as_str());
                                    !shared
                                } else {
                                    true
                                }
                            })
                            .map(|(l, a)| (l.clone(), a.clone()))
                            .collect();
                        pattern_labels_list.push(produced);

                        for (k, v) in &sub.alias_map {
                            self.alias_map.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                        // Collect ALL alias→label pairs from each pattern
                        // (different patterns may use different aliases for the
                        // same label).
                        for (label, alias) in &sub.alias_map {
                            all_alias_pairs.push((label.clone(), alias.clone()));
                        }
                        self.absorb(&sub);
                        compiled_patterns.push(compiled);
                    }
                    // Build the label→alias mapping with ALL aliases.
                    let label_map: Vec<(String, Identifier)> = all_alias_pairs;
                    let match_result_alias = self.next_alias("match");
                    ctx.current = Some(Operator::GremlinMatch {
                        patterns: compiled_patterns,
                        pattern_labels: pattern_labels_list,
                        label_map: label_map.clone(),
                        result_alias: match_result_alias.clone(),
                        child: Box::new(child),
                    });
                    ctx.current_alias = match_result_alias;

                    // Apply not()/where() filter patterns as post-filters on
                    // the match results. Each filter is compiled as a sub-
                    // traversal existence check using Apply + Count + Filter.
                    for fsteps in &filter_pattern_steps {
                        let (is_not, inner_steps) = match fsteps.first() {
                            Some(GremlinStep::Not(inner)) => (true, inner.as_slice()),
                            Some(GremlinStep::Where(inner)) => (false, inner.as_slice()),
                            _ => continue,
                        };
                        let match_child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                        // Determine seed alias from the first As() in the inner steps.
                        let mut sub = self.sub_compiler();
                        // Inject match label→alias bindings into sub compiler
                        for (label, alias) in &label_map {
                            sub.alias_map
                                .entry(label.clone())
                                .or_insert_with(|| alias.clone());
                        }
                        let (seed_alias, compile_steps) = match inner_steps.first() {
                            Some(GremlinStep::As(label)) if sub.alias_map.contains_key(label) => {
                                (sub.alias_map[label].clone(), &inner_steps[1..])
                            }
                            _ => (ctx.current_alias.clone(), inner_steps),
                        };

                        // Endpoint constraint: if the sub-traversal ends with
                        // as("label") referencing a bound match variable, the
                        // result must equal that variable's value.
                        let endpoint_target =
                            if let Some(GremlinStep::As(label)) = compile_steps.last() {
                                // Look up the label in the match label_map (use
                                // the alias that was bound by the join patterns).
                                label_map
                                    .iter()
                                    .find(|(l, _)| l == label)
                                    .map(|(_, a)| a.clone())
                            } else {
                                None
                            };

                        let inner_op = sub.compile_seeded(compile_steps, &seed_alias)?;
                        self.absorb(&sub);
                        let sub_result_alias = sub.last_alias.clone();

                        let inner_op = if let Some(target_alias) = endpoint_target {
                            Operator::Filter {
                                predicate: Expr::BinaryOp {
                                    op: BinaryOp::Eq,
                                    left: Box::new(Expr::Variable(sub_result_alias.clone())),
                                    right: Box::new(Expr::Variable(target_alias)),
                                },
                                child: Box::new(inner_op),
                            }
                        } else {
                            inner_op
                        };

                        let cnt_alias = self.next_alias("mcnt");
                        let count_op = Operator::Aggregate {
                            keys: vec![],
                            aggregations: vec![(
                                cypher_core::planner::AggFunction::Count,
                                Expr::Variable(sub_result_alias),
                                None,
                                false,
                                cnt_alias.clone(),
                            )],
                            child: Box::new(inner_op),
                        };

                        let apply = Operator::Apply {
                            outer: Box::new(match_child),
                            inner: Box::new(count_op),
                        };

                        let predicate = if is_not {
                            Expr::BinaryOp {
                                op: BinaryOp::Eq,
                                left: Box::new(Expr::Variable(cnt_alias)),
                                right: Box::new(Expr::Integer(0)),
                            }
                        } else {
                            Expr::BinaryOp {
                                op: BinaryOp::Gt,
                                left: Box::new(Expr::Variable(cnt_alias)),
                                right: Box::new(Expr::Integer(0)),
                            }
                        };
                        ctx.current = Some(Operator::Filter {
                            predicate,
                            child: Box::new(apply),
                        });
                    }
                }

                GremlinStep::Project(keys) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    ctx.pre_project_alias = ctx.current_alias.clone();
                    let mut items: Vec<(Expr, Option<Identifier>)> = keys
                        .iter()
                        .map(|k| {
                            (
                                Expr::Property(
                                    Box::new(Expr::Variable(ctx.current_alias.clone())),
                                    SmolStr::new(k),
                                ),
                                Some(Identifier::new(k.clone())),
                            )
                        })
                        .collect();
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    ctx.pending_project = true;
                    ctx.project_keys = keys.clone();
                    ctx.project_by_count = 0;
                }

                GremlinStep::Identity => {
                    // No-op: identity() passes through the ctx.current traverser unchanged.
                    if ctx.current.is_none() {
                        return Err(CompileError::MissingChild);
                    }
                }

                GremlinStep::Element => {
                    // element() — extract ~element from property Map objects.
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let elem_alias = self.next_alias("elem");
                    let mut items = vec![(
                        Expr::Property(
                            Box::new(Expr::Variable(ctx.current_alias.clone())),
                            SmolStr::new("~element"),
                        ),
                        Some(elem_alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(elem_alias.clone()))),
                        child: Box::new(Operator::Project {
                            items,
                            child: Box::new(child),
                        }),
                    });
                    ctx.current_alias = elem_alias;
                }

                GremlinStep::Index | GremlinStep::IndexWithMap => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let out_alias = self.next_alias("idx");
                    let with_map = matches!(step, GremlinStep::IndexWithMap);
                    ctx.current = Some(Operator::GremlinIndex {
                        child: Box::new(child),
                        alias: ctx.current_alias.clone(),
                        out_alias: out_alias.clone(),
                        with_map,
                    });
                    ctx.current_alias = out_alias;
                }

                GremlinStep::Tail(n) => {
                    // tail(n): take last n rows (preserving existing order).
                    // Uses a gremlin_tail marker so the executor skips early-
                    // termination and takes from the end of the materialized
                    // result set rather than the beginning.
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    ctx.current = Some(Operator::Limit {
                        count: Expr::FunctionCall {
                            name: SmolStr::new("gremlin_tail"),
                            args: vec![Expr::Integer(*n)],
                            distinct: false,
                        },
                        child: Box::new(child),
                    });
                }

                GremlinStep::TailLocal(n) => {
                    // Flush pending order(Scope.local) so the sort is applied
                    // before taking the tail.
                    if ctx.pending_order && ctx.pending_order_local {
                        if let Some(child) = ctx.current.take() {
                            let alias = self.next_alias("srtl");
                            let mut items = vec![(
                                Expr::FunctionCall {
                                    name: SmolStr::new("gremlin_sort_local"),
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
                        ctx.pending_order = false;
                        ctx.pending_order_local = false;
                    }
                    // tail(Scope.local, n): take last n elements of list values
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("tail");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_tail_local"),
                            args: vec![Expr::Variable(ctx.current_alias.clone()), Expr::Integer(*n)],
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

                GremlinStep::Skip(n) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    ctx.current = Some(Operator::Skip {
                        count: Expr::Integer(*n),
                        child: Box::new(child),
                    });
                }

                GremlinStep::ElementMap(keys) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("emap");
                    // elementMap as a function call that returns a Map value
                    let mut args = vec![Expr::Variable(ctx.current_alias.clone())];
                    for k in keys {
                        args.push(Expr::Str(k.as_str().into()));
                    }
                    let fc = Expr::FunctionCall {
                        name: SmolStr::new("gremlin_elementmap"),
                        args,
                        distinct: false,
                    };
                    let mut items = vec![(fc, Some(alias.clone()))];
                    items.extend(self.label_carry_items());
                    ctx.current_alias = alias;
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                }

                GremlinStep::HasKey(keys) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    // After properties(), ctx.current alias is a property Map with ~key.
                    // Check if ~key matches any of the given keys.
                    let predicates: Vec<Expr> = keys
                        .iter()
                        .map(|k| {
                            // Check both: the element has the property (node/edge context)
                            // AND the property Map ~key matches (properties() context).
                            // Use ~key field for property Map objects.
                            Expr::BinaryOp {
                                op: BinaryOp::Eq,
                                left: Box::new(Expr::Property(
                                    Box::new(Expr::Variable(ctx.current_alias.clone())),
                                    SmolStr::new("~key"),
                                )),
                                right: Box::new(Expr::Str(k.as_str().into())),
                            }
                        })
                        .collect();
                    // Any key matches (OR), or if checking on element, all must exist (AND).
                    // In Gremlin, hasKey on properties filters for any of the keys.
                    let predicate = if keys.len() == 1 {
                        // Single key: also support element context
                        Expr::BinaryOp {
                            op: BinaryOp::Or,
                            left: Box::new(predicates[0].clone()),
                            right: Box::new(Expr::IsNotNull(Box::new(Expr::Property(
                                Box::new(Expr::Variable(ctx.current_alias.clone())),
                                SmolStr::new(&keys[0]),
                            )))),
                        }
                    } else {
                        predicates
                            .into_iter()
                            .reduce(|a, b| Expr::BinaryOp {
                                op: BinaryOp::Or,
                                left: Box::new(a),
                                right: Box::new(b),
                            })
                            .unwrap_or(Expr::Bool(true))
                    };
                    ctx.current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(child),
                    });
                }

                GremlinStep::HasValue(values) => {
                    // hasValue filters by ~value (property context) or ctx.current value.
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    // Try ~value for property Map context, fall back to ctx.current alias.
                    let tilde_val = Expr::Property(
                        Box::new(Expr::Variable(ctx.current_alias.clone())),
                        SmolStr::new("~value"),
                    );
                    if values.len() == 1 {
                        // Check ~value == val OR ctx.current == val
                        let val_expr = gremlin_value_to_expr(&values[0]);
                        let pred_tilde = Expr::BinaryOp {
                            op: BinaryOp::Eq,
                            left: Box::new(tilde_val),
                            right: Box::new(val_expr.clone()),
                        };
                        let pred_direct = Expr::BinaryOp {
                            op: BinaryOp::Eq,
                            left: Box::new(Expr::Variable(ctx.current_alias.clone())),
                            right: Box::new(val_expr),
                        };
                        ctx.current = Some(Operator::Filter {
                            predicate: Expr::BinaryOp {
                                op: BinaryOp::Or,
                                left: Box::new(pred_tilde),
                                right: Box::new(pred_direct),
                            },
                            child: Box::new(child),
                        });
                    } else if !values.is_empty() {
                        // Filter null values out, then check ~value IN [non-null values]
                        // OR ~value IS NULL if null is in the list.
                        let non_null: Vec<Expr> = values
                            .iter()
                            .filter(|v| !matches!(v, GremlinValue::Null))
                            .map(gremlin_value_to_expr)
                            .collect();
                        let pred_in = if !non_null.is_empty() {
                            Some(Expr::BinaryOp {
                                op: BinaryOp::In,
                                left: Box::new(tilde_val.clone()),
                                right: Box::new(Expr::List(non_null)),
                            })
                        } else {
                            None
                        };
                        let predicate = match pred_in {
                            Some(pred) => pred,
                            None => Expr::Bool(false),
                        };
                        ctx.current = Some(Operator::Filter {
                            predicate,
                            child: Box::new(child),
                        });
                    } else {
                        ctx.current = Some(child);
                    }
                }

                GremlinStep::HasValuePredicate(pred) => {
                    // hasValue(P.gt(30)) — filter on ~value (property context) or ctx.current value.
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let tilde_val = Expr::Property(
                        Box::new(Expr::Variable(ctx.current_alias.clone())),
                        SmolStr::new("~value"),
                    );
                    let predicate = predicate_to_prop_expr(pred, tilde_val);
                    ctx.current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(child),
                    });
                }

                GremlinStep::Optional(opt_steps) => {
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
                }

                GremlinStep::SimplePath => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    ctx.current = Some(Operator::GremlinSimplePath {
                        child: Box::new(child),
                        path_aliases: ctx.path_aliases.clone(),
                        by_keys: vec![],
                        from_label: None,
                        to_label: None,
                    });
                }

                GremlinStep::CyclicPath => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    ctx.current = Some(Operator::GremlinCyclicPath {
                        child: Box::new(child),
                        path_aliases: ctx.path_aliases.clone(),
                        by_keys: vec![],
                        from_label: None,
                        to_label: None,
                    });
                    ctx.pending_path = true;
                    ctx.path_by_keys.clear();
                }

                GremlinStep::Coin(probability) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let rand_expr = if let Some(seed) = self.seed {
                        // Seeded: use Java-compatible LCG for deterministic results
                        Expr::FunctionCall {
                            name: SmolStr::new("java_random_next_double"),
                            args: vec![Expr::Integer(seed)],
                            distinct: false,
                        }
                    } else {
                        Expr::FunctionCall {
                            name: SmolStr::new("rand"),
                            args: vec![],
                            distinct: false,
                        }
                    };
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::BinaryOp {
                            op: BinaryOp::Lt,
                            left: Box::new(rand_expr),
                            right: Box::new(Expr::Float(*probability)),
                        },
                        child: Box::new(child),
                    });
                }

                GremlinStep::Map(map_steps) => {
                    // map(traversal) — apply sub-traversal to each traverser
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let mut sub = self.sub_compiler();
                    // Pass parent ctx.path_aliases so tree() inside local() can
                    // access the full traversal path.
                    sub.outer_path_aliases = ctx.path_aliases.clone();
                    let map_op = sub.compile_seeded(map_steps, &ctx.current_alias)?;
                    self.absorb(&sub);
                    // Merge sub-compiler's path tracking into local ctx.path_aliases
                    // so path()/simplePath() see aliases from the map body
                    // (e.g., local(out().in().simplePath()).path()).
                    for alias in &sub.path_tracking {
                        if !ctx.path_aliases.contains(alias) {
                            ctx.path_aliases.push(alias.clone());
                        }
                    }
                    ctx.current_alias = sub.last_alias.clone();
                    ctx.current = Some(Operator::Apply {
                        outer: Box::new(child),
                        inner: Box::new(map_op),
                    });
                }

                GremlinStep::FlatMap(fm_steps) => {
                    // flatMap(traversal) — apply and flatten
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let mut sub = self.sub_compiler();
                    let fm_op = sub.compile_seeded(fm_steps, &ctx.current_alias)?;
                    self.absorb(&sub);
                    ctx.current_alias = sub.last_alias.clone();
                    ctx.current = Some(Operator::Apply {
                        outer: Box::new(child),
                        inner: Box::new(fm_op),
                    });
                }

                GremlinStep::FilterTraversal(filter_steps) => {
                    // filter(traversal) — keep traversers where sub-traversal is true
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    // Try to extract a predicate from the inner steps
                    let pred = steps_to_predicate(
                        filter_steps,
                        &ctx.current_alias,
                        &self.alias_map,
                        &self.aggregate_keys,
                    );
                    // If steps_to_predicate returned true (couldn't convert) and
                    // there are actual steps, fall back to sub-traversal existence
                    // check via Apply + Count + Filter (same pattern as Where).
                    if pred == Expr::Bool(true) && !filter_steps.is_empty() {
                        let mut sub = self.sub_compiler();
                        // If the child is an Unwind, the seed element is a map
                        // entry — propagate this so select(Column.values) in
                        // the sub-traversal extracts the value directly.
                        if matches!(&child, Operator::Unwind { .. }) || self.seed_is_map_entry {
                            sub.seed_is_map_entry = true;
                        }
                        let inner_op = sub.compile_seeded(filter_steps, &ctx.current_alias)?;
                        // filter(__...) is a scoped existence check. Labels and
                        // aliases created only for the filter body must not
                        // leak back into the outer traversal, or later
                        // select(Pop...) steps can read those internal aliases
                        // as if they were outer traverser bindings.
                        self.alias_counter = sub.alias_counter;
                        let sub_result_alias = sub.last_alias.clone();
                        let cnt_alias = self.next_alias("fcnt");
                        let count_op = Operator::Aggregate {
                            keys: vec![],
                            aggregations: vec![(
                                cypher_core::planner::AggFunction::Count,
                                Expr::Variable(sub_result_alias),
                                None,
                                false,
                                cnt_alias.clone(),
                            )],
                            child: Box::new(inner_op),
                        };
                        let apply = Operator::Apply {
                            outer: Box::new(child),
                            inner: Box::new(count_op),
                        };
                        ctx.current = Some(Operator::Filter {
                            predicate: Expr::BinaryOp {
                                op: BinaryOp::Gt,
                                left: Box::new(Expr::Variable(cnt_alias)),
                                right: Box::new(Expr::Integer(0)),
                            },
                            child: Box::new(apply),
                        });
                    } else {
                        ctx.current = Some(Operator::Filter {
                            predicate: pred,
                            child: Box::new(child),
                        });
                    }
                }

                GremlinStep::None => {
                    // none() — produces no results (empty)
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::Bool(false),
                        child: Box::new(child),
                    });
                }

                GremlinStep::Barrier => {
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
                }

                GremlinStep::BarrierNormSack => {
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
                }

                GremlinStep::Sample(n) => {
                    // sample(n) is lowered when we know whether a by() modulator follows.
                    ctx.pending_sample = true;
                    ctx.sample_by_consumed = false;
                    ctx.pending_sample_count = *n;
                    ctx.pending_sample_weight = None;
                }

                GremlinStep::SampleLocal(n) => {
                    // sample(Scope.local, n) samples within the ctx.current list/map value.
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    ctx.current = Some(Operator::Project {
                        items: vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_sample_local"),
                                args: vec![
                                    Expr::Variable(ctx.current_alias.clone()),
                                    Expr::Integer(*n),
                                    self.seed.map_or(Expr::Null, Expr::Integer),
                                ],
                                distinct: false,
                            },
                            Some(ctx.current_alias.clone()),
                        )],
                        child: Box::new(child),
                    });
                }

                GremlinStep::Discard => {
                    // discard() — consume traversal, return empty
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::Bool(false),
                        child: Box::new(child),
                    });
                }

                GremlinStep::Loops(ref label) => {
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

                GremlinStep::Sack => {
                    // sack() — return the sack value from the __sack column
                    let child = ctx.current.take().unwrap_or(Operator::SingleRow);
                    let alias = self.next_alias("sack");
                    let sack_var = Identifier::new("__sack");
                    let mut items = vec![(Expr::Variable(sack_var), Some(alias.clone()))];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    ctx.current_alias = alias;
                }

                GremlinStep::SackOp(op) => {
                    // sack(operator) — apply: __sack = __sack OP current_value
                    // Must preserve ctx.current_alias and all label columns.
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let sack_var = Identifier::new("__sack");
                    let bin_op = match op.as_str() {
                        "sum" | "add" => BinaryOp::Add,
                        "minus" | "sub" => BinaryOp::Sub,
                        "mult" | "multiply" => BinaryOp::Mul,
                        "div" | "divide" => BinaryOp::Div,
                        "and" => BinaryOp::And,
                        "or" => BinaryOp::Or,
                        "assign" => {
                            let mut items = vec![
                                (Expr::Variable(ctx.current_alias.clone()), Some(sack_var)),
                                (
                                    Expr::Variable(ctx.current_alias.clone()),
                                    Some(ctx.current_alias.clone()),
                                ),
                            ];
                            items.extend(self.label_carry_items());
                            ctx.current = Some(Operator::Project {
                                items,
                                child: Box::new(child),
                            });
                            ctx.pending_sack_op = true;
                            continue;
                        }
                        _ => BinaryOp::Add,
                    };
                    let mut items = vec![
                        (
                            Expr::BinaryOp {
                                op: bin_op,
                                left: Box::new(Expr::Variable(sack_var.clone())),
                                right: Box::new(Expr::Variable(ctx.current_alias.clone())),
                            },
                            Some(sack_var),
                        ),
                        (
                            Expr::Variable(ctx.current_alias.clone()),
                            Some(ctx.current_alias.clone()),
                        ),
                    ];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    ctx.pending_sack_op = true;
                }

                GremlinStep::WithSack(val, ref merge_op) => {
                    ctx.pending_sack_init = Some(gremlin_value_to_expr(val));
                    ctx.pending_sack_merge_op = merge_op.clone();
                }

                GremlinStep::WithBulkFalse => {
                    ctx.with_bulk_false = true;
                }

                GremlinStep::WithSideEffect(key, val, ref reduce_op) => {
                    // withSideEffect(key, value[, operator]) — register a named side-effect
                    let se_alias = Identifier::new(format!("__side_effect_{}", key));
                    // Collect carry items BEFORE inserting the new alias to avoid
                    // a self-referencing carry item that would try to read the
                    // column we're about to create.
                    let carry = self.label_carry_items();
                    self.alias_map.insert(key.clone(), se_alias.clone());
                    // Store the literal value for compile-time key resolution
                    self.side_effect_values.insert(key.clone(), val.clone());
                    // Track set-semantics keys (initialized via `{...}` notation)
                    if reduce_op.as_deref() == Some("set") {
                        self.set_keys.insert(key.clone());
                    }
                    // For reduce operators, store in compiler state for Cap to use.
                    // Still inject the initial value as a column so downstream
                    // select/where can resolve it (e.g., where(within('a'))).
                    if let Some(op) = reduce_op {
                        if op == "assign" {
                            // Operator.assign: track via reductions so GremlinAggregate
                            // can detect it. No numeric init needed.
                            self.reductions.insert(key.clone(), (op.clone(), 0.0));
                        } else if op != "set" {
                            // "set" is tracked via set_keys, not reductions
                            let init_val = match val {
                                GremlinValue::Integer(n) => *n as f64,
                                GremlinValue::Float(f) => *f,
                                GremlinValue::Bool(b) if *b => 1.0,
                                _ => 0.0,
                            };
                            self.reductions.insert(key.clone(), (op.clone(), init_val));
                        }
                    }
                    // Always inject the side-effect value as a column so
                    // downstream select/where/property can resolve it.
                    let child = ctx.current.take().unwrap_or(Operator::SingleRow);
                    let val_expr = gremlin_value_to_expr(val);
                    let mut items = vec![(val_expr, Some(se_alias))];
                    items.extend(carry);
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                }

                GremlinStep::ProductiveByStrategy => {
                    self.productive_by = true;
                }

                GremlinStep::SeedStrategy(seed) => {
                    self.seed = Some(*seed);
                }

                GremlinStep::ReadOnlyStrategy => {
                    self.read_only = true;
                }

                GremlinStep::ReservedKeysVerificationStrategy(keys) => {
                    self.reserved_keys = Some(keys.clone());
                }

                GremlinStep::EdgeLabelVerificationStrategy => {
                    self.edge_label_verify = true;
                }

                GremlinStep::WithoutConnectiveStrategy => {
                    // Handled during preprocessing; no compiler state needed.
                }

                GremlinStep::DedupLocal(keys) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("dedup_local");
                    let mut args = vec![Expr::Variable(ctx.current_alias.clone())];
                    for k in keys {
                        args.push(Expr::Str(k.clone().into_boxed_str()));
                    }
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_dedup_local"),
                            args,
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

                GremlinStep::Option(token, branch_steps) => {
                    // Handle Merge.* tokens for mergeV/mergeE options
                    let is_merge_option = matches!(
                        token,
                        GremlinValue::String(s) if s.starts_with("Merge.")
                    );
                    if is_merge_option {
                        // mergeV/mergeE options — no-op for now
                    } else if let Some(Operator::ChooseMap {
                        ref mut options, ..
                    }) = ctx.current
                    {
                        let mut sub = self.sub_compiler();
                        let branch_op = sub.compile_seeded(branch_steps, &ctx.current_alias)?;
                        let branch_alias = sub.last_alias.clone();
                        self.absorb(&sub);
                        let match_val = match token {
                            GremlinValue::String(s) if s == "none" => None,
                            _ => Some(gremlin_value_to_value(token)),
                        };
                        options.push((match_val, Box::new(branch_op), Some(branch_alias)));
                    } else if ctx.current.is_none() {
                        return Err(CompileError::MissingChild);
                    }
                    // If ctx.current is not ChooseMap or Merge, option is ignored (no-op).
                }

                // ── String steps ─────────────────────────────────────────────────
                GremlinStep::ToLower => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("str");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_toLower"),
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

                GremlinStep::ToUpper => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("str");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_toUpper"),
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

                GremlinStep::Trim => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("str");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_trim"),
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

                GremlinStep::LTrim => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("str");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_lTrim"),
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

                GremlinStep::RTrim => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("str");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_rTrim"),
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

                GremlinStep::Length => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("len");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_length"),
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

                GremlinStep::Reverse => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("rev");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_reverse"),
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

                GremlinStep::Replace(old, new) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("str");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_replace"),
                            args: vec![
                                Expr::Variable(ctx.current_alias.clone()),
                                Expr::Str(old.clone().into_boxed_str()),
                                Expr::Str(new.clone().into_boxed_str()),
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
                }

                GremlinStep::Split(sep) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("split");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_split"),
                            args: vec![
                                Expr::Variable(ctx.current_alias.clone()),
                                Expr::Str(sep.clone().into_boxed_str()),
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
                }

                GremlinStep::Substring(start, len) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("substr");
                    let mut args =
                        vec![Expr::Variable(ctx.current_alias.clone()), Expr::Integer(*start)];
                    if let Some(l) = len {
                        args.push(Expr::Integer(*l));
                    }
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_substring"),
                            args,
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

                GremlinStep::SubstringLocal(start, len) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("substrl");
                    let mut args =
                        vec![Expr::Variable(ctx.current_alias.clone()), Expr::Integer(*start)];
                    if let Some(l) = len {
                        args.push(Expr::Integer(*l));
                    }
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_substring_local"),
                            args,
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

                GremlinStep::Concat(traversals) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("concat");
                    // Build concat args: ctx.current value + each sub-traversal result
                    let mut args = vec![Expr::Variable(ctx.current_alias.clone())];
                    let mut base = child;
                    for trav in traversals {
                        if trav.is_empty() {
                            // Empty traversal → empty string
                            args.push(Expr::Str("".into()));
                            continue;
                        }
                        // Single constant → inline
                        if trav.len() == 1 {
                            if let GremlinStep::Constant(v) = &trav[0] {
                                args.push(gremlin_value_to_expr(v));
                                continue;
                            }
                        }
                        // Sub-traversal: compile and wrap in Apply.
                        // Limit inner to 1 result per outer row (Gremlin concat
                        // uses only the first value from inner traversals).
                        // Use compile_from with a seed so inject() adds to the
                        // incoming stream rather than replacing it.
                        let mut sub = self.sub_compiler();
                        let sub_op = if matches!(trav.first(), Some(GremlinStep::Inject(_))) {
                            sub.compile_from(
                                trav,
                                Some(Operator::SingleRow),
                                ctx.current_alias.clone(),
                                vec![ctx.current_alias.clone()],
                            )?
                        } else {
                            sub.compile_seeded(trav, &ctx.current_alias)?
                        };
                        let sub_alias = sub.last_alias.clone();
                        self.absorb(&sub);
                        let limited = Operator::Limit {
                            count: Expr::Integer(1),
                            child: Box::new(sub_op),
                        };
                        base = Operator::Apply {
                            outer: Box::new(base),
                            inner: Box::new(limited),
                        };
                        args.push(Expr::Variable(sub_alias));
                    }
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_concat"),
                            args,
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(base),
                    });
                    ctx.current_alias = alias;
                }

                GremlinStep::Conjoin(sep) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("join");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_conjoin"),
                            args: vec![
                                Expr::Variable(ctx.current_alias.clone()),
                                Expr::Str(sep.clone().into_boxed_str()),
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
                }

                GremlinStep::Format(pattern) => {
                    if pattern.contains("%{_}") {
                        // Pattern has positional placeholders — defer until by() modulations collected
                        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                        ctx.pending_format = true;
                        ctx.format_pattern = pattern.clone();
                        ctx.format_pre_alias = ctx.current_alias.clone();
                        ctx.format_by_aliases.clear();
                        ctx.current = Some(child);
                    } else {
                        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                        let alias = self.next_alias("fmt");
                        // Build a map of as-label values for %{label} resolution
                        let label_pairs: Vec<(SmolStr, Expr)> = self
                            .alias_map
                            .iter()
                            .map(|(label, ident)| {
                                (SmolStr::new(label), Expr::Variable(ident.clone()))
                            })
                            .collect();
                        let mut args = vec![
                            Expr::Variable(ctx.current_alias.clone()),
                            Expr::Str(pattern.clone().into_boxed_str()),
                        ];
                        if !label_pairs.is_empty() {
                            args.push(Expr::MapLiteral(label_pairs));
                        }
                        let mut items = vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_format"),
                                args,
                                distinct: false,
                            },
                            Some(alias.clone()),
                        )];
                        items.extend(self.label_carry_items());
                        let project = Operator::Project {
                            items,
                            child: Box::new(child),
                        };
                        // Filter out traversers where format couldn't resolve all placeholders
                        ctx.current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                            child: Box::new(project),
                        });
                        ctx.current_alias = alias;
                    }
                }

                GremlinStep::StringOpLocal(fn_name) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("sloc");
                    // Parse function name and optional extra args from \0-separated string
                    let parts: Vec<&str> = fn_name.split('\0').collect();
                    let func_name = parts[0];
                    let mut args = vec![Expr::Variable(ctx.current_alias.clone())];
                    for extra in &parts[1..] {
                        args.push(Expr::Str(extra.to_string().into_boxed_str()));
                    }
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new(func_name),
                            args,
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

                GremlinStep::AsString => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("str");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_asstring"),
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

                GremlinStep::AsStringLocal => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("strl");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_asstring_local"),
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

                GremlinStep::AsNumber => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("num");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_asnumber"),
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

                GremlinStep::AsNumberTyped(ref target_type) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("num");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_asnumber_typed"),
                            args: vec![
                                Expr::Variable(ctx.current_alias.clone()),
                                Expr::Str(target_type.clone().into_boxed_str()),
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
                    self.numeric_subtype = Some(target_type.clone());
                }

                GremlinStep::AsBool => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("bool");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_asBool"),
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

                GremlinStep::Math(expr_str) => {
                    // Set ctx.pending_math so that a following by() can resolve variables
                    ctx.pending_math = true;
                    ctx.pending_math_expr = expr_str.clone();
                    ctx.pre_math_alias = ctx.current_alias.clone();
                }

                GremlinStep::TypeOf => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("type");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_typeof"),
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

                // ── Collection steps ─────────────────────────────────────────────
                GremlinStep::Combine(rhs_steps) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("coll");
                    let (rhs_expr, base) =
                        self.compile_collection_rhs(rhs_steps, &ctx.current_alias, child)?;
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_combine"),
                            args: vec![Expr::Variable(ctx.current_alias.clone()), rhs_expr],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(base),
                    });
                    ctx.current_alias = alias;
                }

                GremlinStep::Intersect(rhs_steps) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("coll");
                    let (rhs_expr, base) =
                        self.compile_collection_rhs(rhs_steps, &ctx.current_alias, child)?;
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_intersect"),
                            args: vec![Expr::Variable(ctx.current_alias.clone()), rhs_expr],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(base),
                    });
                    ctx.current_alias = alias;
                }

                GremlinStep::Difference(rhs_steps) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("coll");
                    let (rhs_expr, base) =
                        self.compile_collection_rhs(rhs_steps, &ctx.current_alias, child)?;
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_difference"),
                            args: vec![Expr::Variable(ctx.current_alias.clone()), rhs_expr],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(base),
                    });
                    ctx.current_alias = alias;
                }

                GremlinStep::Disjunct(rhs_steps) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("coll");
                    let (rhs_expr, base) =
                        self.compile_collection_rhs(rhs_steps, &ctx.current_alias, child)?;
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_disjunct"),
                            args: vec![Expr::Variable(ctx.current_alias.clone()), rhs_expr],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(base),
                    });
                    ctx.current_alias = alias;
                }

                GremlinStep::MergeList(rhs_steps) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("coll");
                    let (rhs_expr, base) =
                        self.compile_collection_rhs(rhs_steps, &ctx.current_alias, child)?;
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_merge"),
                            args: vec![Expr::Variable(ctx.current_alias.clone()), rhs_expr],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(base),
                    });
                    ctx.current_alias = alias;
                }

                GremlinStep::Product(rhs_steps) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("coll");
                    let (rhs_expr, base) =
                        self.compile_collection_rhs(rhs_steps, &ctx.current_alias, child)?;
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_product"),
                            args: vec![Expr::Variable(ctx.current_alias.clone()), rhs_expr],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(base),
                    });
                    ctx.current_alias = alias;
                }

                GremlinStep::All(pred) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let (pred_op_str, pred_val_expr) = predicate_to_args(pred);
                    let pred_fn = Expr::FunctionCall {
                        name: SmolStr::new("gremlin_all"),
                        args: vec![
                            Expr::Variable(ctx.current_alias.clone()),
                            Expr::Str(pred_op_str),
                            pred_val_expr,
                        ],
                        distinct: false,
                    };
                    ctx.current = Some(Operator::Filter {
                        predicate: pred_fn,
                        child: Box::new(child),
                    });
                }

                GremlinStep::Any(pred) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let (pred_op_str, pred_val_expr) = predicate_to_args(pred);
                    let pred_fn = Expr::FunctionCall {
                        name: SmolStr::new("gremlin_any"),
                        args: vec![
                            Expr::Variable(ctx.current_alias.clone()),
                            Expr::Str(pred_op_str),
                            pred_val_expr,
                        ],
                        distinct: false,
                    };
                    ctx.current = Some(Operator::Filter {
                        predicate: pred_fn,
                        child: Box::new(child),
                    });
                }

                GremlinStep::NonePred(pred) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let (pred_op_str, pred_val_expr) = predicate_to_args(pred);
                    let pred_fn = Expr::FunctionCall {
                        name: SmolStr::new("gremlin_none"),
                        args: vec![
                            Expr::Variable(ctx.current_alias.clone()),
                            Expr::Str(pred_op_str),
                            pred_val_expr,
                        ],
                        distinct: false,
                    };
                    ctx.current = Some(Operator::Filter {
                        predicate: pred_fn,
                        child: Box::new(child),
                    });
                }

                // Predicate steps used inside Where/Is — should not appear at top level
                GremlinStep::Eq(_)
                | GremlinStep::Neq(_)
                | GremlinStep::Gt(_)
                | GremlinStep::Gte(_)
                | GremlinStep::Lt(_)
                | GremlinStep::Lte(_)
                | GremlinStep::Within(_)
                | GremlinStep::Without(_) => {
                    return Err(CompileError::InvalidSequence(
                        "predicate steps should not appear at the top level".into(),
                    ));
                }

                // Unsupported steps — parsed successfully but not compilable
                GremlinStep::Tree(key) => {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    // Use outer path aliases when inside local()/map(),
                    // otherwise use the ctx.current ctx.path_aliases.
                    let tree_path_aliases = if !self.outer_path_aliases.is_empty() {
                        self.outer_path_aliases.clone()
                    } else {
                        ctx.path_aliases.clone()
                    };
                    let alias = self.next_alias("tree");
                    ctx.current = Some(Operator::GremlinTree {
                        child: Box::new(child),
                        path_aliases: tree_path_aliases,
                        side_effect_key: key.clone(),
                        alias: alias.clone(),
                    });
                    if let Some(k) = key {
                        self.aggregate_keys.insert(k.clone());
                    } else {
                        // Barrier mode: tree becomes the new traverser value
                        ctx.current_alias = alias;
                    }
                }
                GremlinStep::Subgraph(_) => {
                    return Err(CompileError::UnsupportedStep("subgraph".into()));
                }
                GremlinStep::Call(_) => {
                    return Err(CompileError::UnsupportedStep("call".into()));
                }
                GremlinStep::ShortestPath => {
                    return Err(CompileError::UnsupportedStep("shortestPath".into()));
                }
                GremlinStep::PageRank => {
                    return Err(CompileError::UnsupportedStep("pageRank".into()));
                }
                GremlinStep::ConnectedComponent => {
                    return Err(CompileError::UnsupportedStep("connectedComponent".into()));
                }
                GremlinStep::PeerPressure => {
                    return Err(CompileError::UnsupportedStep("peerPressure".into()));
                }
                GremlinStep::Io(_) => {
                    return Err(CompileError::UnsupportedStep("io".into()));
                }
                GremlinStep::AsDate => {
                    // asDate() — convert epoch-millis integer to Date value.
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("date");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_asdate"),
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
                GremlinStep::DateAdd(..) => {
                    return Err(CompileError::UnsupportedStep("dateAdd".into()));
                }
                GremlinStep::DateDiff(_) => {
                    return Err(CompileError::UnsupportedStep("dateDiff".into()));
                }
                GremlinStep::ToE(_) => {
                    return Err(CompileError::UnsupportedStep("toE".into()));
                }
            }

            // Generic path tracking: if the ctx.current alias changed and this is
            // a path-contributing step, record it for path() reconstruction.
            if self.needs_path_tracking && ctx.current_alias != pre_step_alias {
                let is_modulator = matches!(
                    step,
                    GremlinStep::By(..)
                        | GremlinStep::ByTraversal(..)
                        | GremlinStep::From(_)
                        | GremlinStep::To(_)
                        | GremlinStep::As(_)
                        | GremlinStep::Times(_)
                        | GremlinStep::Until(_)
                        | GremlinStep::Emit
                        | GremlinStep::EmitPred(_)
                        | GremlinStep::Option(..)
                        | GremlinStep::WithSideEffect(..)
                        | GremlinStep::WithSack(..)
                        | GremlinStep::WithBulkFalse
                        | GremlinStep::ProductiveByStrategy
                        | GremlinStep::ReadOnlyStrategy
                        | GremlinStep::SeedStrategy(_)
                        | GremlinStep::ReservedKeysVerificationStrategy(_)
                        | GremlinStep::EdgeLabelVerificationStrategy
                        | GremlinStep::WithoutConnectiveStrategy
                        | GremlinStep::Property(..)
                        | GremlinStep::Inject(_)
                );
                // Don't re-add aliases that are already explicitly tracked
                let already_tracked = ctx.path_aliases.last() == Some(&ctx.current_alias);
                if !is_modulator && !already_tracked {
                    ctx.path_aliases.push(ctx.current_alias.clone());
                    self.path_tracking.push(ctx.current_alias.clone());
                }
            }
        }

        // Flush deferred choose output alias
        if let Some(alias) = ctx.pending_choose_output.take() {
            ctx.current_alias = alias;
        }

        // Flush pending select by-mods (handles by(T.key) as last step)
        if ctx.pending_select && !ctx.select_by_mods.is_empty() {
            self.apply_pending_select_mods(&mut ctx.current, &mut ctx.current_alias, &ctx.select_by_mods)?;
        }

        // Flush accumulated path by-keys into the GremlinPath/CyclicPath operator
        if ctx.pending_path && !ctx.path_by_keys.is_empty() {
            match &mut ctx.current {
                Some(Operator::GremlinPath {
                    ref mut by_keys, ..
                }) => {
                    *by_keys = ctx.path_by_keys;
                }
                Some(Operator::GremlinCyclicPath {
                    ref mut by_keys, ..
                }) => {
                    *by_keys = ctx.path_by_keys;
                }
                Some(Operator::GremlinSimplePath {
                    ref mut by_keys, ..
                }) => {
                    *by_keys = ctx.path_by_keys;
                }
                _ => {}
            }
        }

        // If Order was set but no By followed, apply default sort
        if ctx.pending_order && ctx.pending_order_local {
            // order(Scope.local) with no by() — default sort
            if let Some(child) = ctx.current.take() {
                let alias = self.next_alias("srtl");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_sort_local"),
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
        } else if ctx.pending_order {
            if let Some(child) = ctx.current.take() {
                ctx.current = Some(Operator::Sort {
                    items: vec![(Expr::Variable(ctx.current_alias.clone()), true)],
                    gremlin_order: true,
                    child: Box::new(child),
                });
            }
        }

        // If a withSack() was present, inject __sack initialization into the
        // bottom-most leaf operator of the tree.
        if let Some(init_expr) = ctx.pending_sack_init {
            if let Some(ref mut op) = ctx.current {
                inject_sack_init(op, &init_expr);
            }
        }

        // Validate: emit/until/times without repeat is an error
        if ctx.pending_emit || ctx.pending_until.is_some() || ctx.pending_times.is_some() {
            return Err(CompileError::InvalidSequence(
                "The repeat()-traversal was not defined".into(),
            ));
        }

        // Flush any remaining addE defaults (traversal ends with addE group)
        if let Some(ref traverser) = ctx.adde_traverser {
            if let Some(ref mut op) = ctx.current {
                rewrite_create_alias(op, "__src", traverser);
                rewrite_create_alias(op, "__dst", traverser);
            }
        }

        // Flush pending math without by() modulation
        if ctx.pending_math {
            let child = ctx.current.take().unwrap_or(Operator::SingleRow);
            let alias = self.next_alias("math");
            let mut args = vec![
                Expr::Variable(ctx.pre_math_alias.clone()),
                Expr::Str(ctx.pending_math_expr.clone().into_boxed_str()),
            ];
            for var_name in extract_math_variables(&ctx.pending_math_expr) {
                let se_key = format!("__side_effect_{}", var_name);
                if let Some(se_ident) = self.alias_map.get(&var_name) {
                    args.push(Expr::Str(var_name.into_boxed_str()));
                    args.push(Expr::Variable(se_ident.clone()));
                } else if self.side_effect_values.contains_key(&var_name) {
                    args.push(Expr::Str(var_name.into_boxed_str()));
                    args.push(Expr::Variable(Identifier::new(se_key)));
                }
            }
            let mut items = vec![(
                Expr::FunctionCall {
                    name: SmolStr::new("gremlin_math"),
                    args,
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

        if ctx.pending_sample {
            let child = ctx.current.take().unwrap_or(Operator::SingleRow);
            ctx.current = Some(Operator::GremlinSample {
                count: ctx.pending_sample_count as usize,
                weight: ctx.pending_sample_weight,
                seed: self.seed,
                child: Box::new(child),
            });
        }

        // Finalize any remaining format with by() modulations
        if ctx.pending_format && !ctx.format_by_aliases.is_empty() {
            let child = ctx.current.take().unwrap_or(Operator::SingleRow);
            let alias = self.next_alias("fmt");
            let mut args = vec![
                Expr::Variable(ctx.format_pre_alias.clone()),
                Expr::Str(ctx.format_pattern.clone().into_boxed_str()),
            ];
            for by_alias in &ctx.format_by_aliases {
                args.push(Expr::Variable(by_alias.clone()));
            }
            let mut items = vec![(
                Expr::FunctionCall {
                    name: SmolStr::new("gremlin_format_by"),
                    args,
                    distinct: false,
                },
                Some(alias.clone()),
            )];
            items.extend(self.label_carry_items());
            let project = Operator::Project {
                items,
                child: Box::new(child),
            };
            ctx.current = Some(Operator::Filter {
                predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                child: Box::new(project),
            });
            ctx.current_alias = alias;
        }

        if !ctx.pending_where_by_targets.is_empty() && !ctx.pending_where_by_modulators.is_empty() {
            if let Some(Operator::Filter {
                ref mut predicate, ..
            }) = ctx.current
            {
                finalize_where_by_modulation(
                    predicate,
                    &ctx.pending_where_by_targets,
                    &ctx.pending_where_by_modulators,
                    self.productive_by,
                );
            }
        }

        self.last_alias = ctx.current_alias;
        ctx.current.ok_or(CompileError::MissingChild)
    }
}
