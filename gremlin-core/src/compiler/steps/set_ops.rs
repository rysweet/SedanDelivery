//! `set_ops` category step compilation arms extracted from `dispatch.rs`.
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
    pub(super) fn compile_set_ops_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
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

            _ => unreachable!(
                "compile_set_ops_step received non-set_ops variant: {:?}",
                step
            ),
        }
        #[allow(unreachable_code)]
        Ok(StepFlow::Proceed)
    }
}
