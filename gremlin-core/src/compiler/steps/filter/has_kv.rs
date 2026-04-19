//! `hasKey`, `hasValue`, and `hasValue(predicate)` arms.
//!
//! Pure mechanical extraction from the original `filter.rs` — no behavior changes.

#![allow(unused_imports)]

use cypher_core::ast::{BinaryOp, Expr};
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::helpers::*;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::Predicate;
use crate::types::GremlinValue;

impl GremlinCompiler {
    pub(super) fn compile_has_key(
        &mut self,
        keys: &[String],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
        Ok(())
    }

    pub(super) fn compile_has_value(
        &mut self,
        values: &[GremlinValue],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
        Ok(())
    }

    pub(super) fn compile_has_value_predicate(
        &mut self,
        pred: &Predicate,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
        Ok(())
    }
}
