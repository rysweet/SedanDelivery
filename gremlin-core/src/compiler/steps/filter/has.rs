//! `has*` family arms: HasLabel, Has, HasPredicate, HasId, HasIdPredicate.
//!
//! Pure mechanical extraction from the original `filter.rs` — no behavior
//! changes. Each method body is verbatim from the corresponding case in the
//! original monolithic match.

#![allow(unused_imports)]

use cypher_core::ast::{BinaryOp, Expr, Label};
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::helpers::*;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::Predicate;
use crate::types::GremlinValue;

impl GremlinCompiler {
    pub(super) fn compile_has_label(
        &mut self,
        labels: &[String],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
        Ok(())
    }

    pub(super) fn compile_has(
        &mut self,
        key: &str,
        value: Option<&GremlinValue>,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
        Ok(())
    }

    pub(super) fn compile_has_predicate(
        &mut self,
        key: &str,
        pred: &Predicate,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let prop_expr = gremlin_has_key_expr(key, &ctx.current_alias);
        let predicate = predicate_to_prop_expr(pred, prop_expr);
        ctx.current = Some(Operator::Filter {
            predicate,
            child: Box::new(child),
        });
        Ok(())
    }

    pub(super) fn compile_has_id(
        &mut self,
        ids: &[GremlinValue],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
            let id_exprs: Vec<Expr> = flat_ids.iter().map(gremlin_value_to_id_expr).collect();
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
        Ok(())
    }

    pub(super) fn compile_has_id_predicate(
        &mut self,
        pred: &Predicate,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
        Ok(())
    }
}
