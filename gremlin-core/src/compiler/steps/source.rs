//! `source` category step compilation arms extracted from `dispatch.rs`.
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
    pub(super) fn compile_source_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
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
                                    let mut items =
                                        vec![(Expr::Variable(alias), Some(union_alias.clone()))];
                                    // Carry side-effect columns into injected
                                    // rows so select("k") can find them.
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

            _ => unreachable!(
                "compile_source_step received non-source variant: {:?}",
                step
            ),
        }
        #[allow(unreachable_code)]
        Ok(StepFlow::Proceed)
    }
}
