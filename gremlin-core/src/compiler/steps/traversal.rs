//! `traversal` category step compilation arms extracted from `dispatch.rs`.
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
    pub(super) fn compile_traversal_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
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

            GremlinStep::ToE(_) => {
                return Err(CompileError::UnsupportedStep("toE".into()));
            }
            _ => unreachable!(
                "compile_traversal_step received non-traversal variant: {:?}",
                step
            ),
        }
        #[allow(unreachable_code)]
        Ok(StepFlow::Proceed)
    }
}
