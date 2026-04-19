//! `sack()`, `sack(operator)`, `withSack()`, and `withSideEffect()` step compilation.

#![allow(unused_imports)]

use cypher_core::ast::{BinaryOp, Expr, Identifier};
use cypher_core::planner::Operator;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::helpers::*;
use super::super::super::{CompileError, GremlinCompiler};
use super::super::StepFlow;
use crate::types::GremlinValue;

impl GremlinCompiler {
    pub(super) fn compile_sack(&mut self, ctx: &mut CompileFromContext) {
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

    pub(super) fn compile_sack_op(
        &mut self,
        op: &str,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        // sack(operator) — apply: __sack = __sack OP current_value
        // Must preserve ctx.current_alias and all label columns.
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let sack_var = Identifier::new("__sack");
        let bin_op = match op {
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
                return Ok(StepFlow::Continue);
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
        Ok(StepFlow::Proceed)
    }

    pub(super) fn compile_with_sack(
        &mut self,
        val: &GremlinValue,
        merge_op: &Option<String>,
        ctx: &mut CompileFromContext,
    ) {
        ctx.pending_sack_init = Some(gremlin_value_to_expr(val));
        ctx.pending_sack_merge_op = merge_op.clone();
    }

    pub(super) fn compile_with_side_effect(
        &mut self,
        key: &str,
        val: &GremlinValue,
        reduce_op: &Option<String>,
        ctx: &mut CompileFromContext,
    ) {
        // withSideEffect(key, value[, operator]) — register a named side-effect
        let se_alias = Identifier::new(format!("__side_effect_{}", key));
        // Collect carry items BEFORE inserting the new alias to avoid
        // a self-referencing carry item that would try to read the
        // column we're about to create.
        let carry = self.label_carry_items();
        self.alias_map.insert(key.to_string(), se_alias.clone());
        // Store the literal value for compile-time key resolution
        self.side_effect_values.insert(key.to_string(), val.clone());
        // Track set-semantics keys (initialized via `{...}` notation)
        if reduce_op.as_deref() == Some("set") {
            self.set_keys.insert(key.to_string());
        }
        // For reduce operators, store in compiler state for Cap to use.
        // Still inject the initial value as a column so downstream
        // select/where can resolve it (e.g., where(within('a'))).
        if let Some(op) = reduce_op {
            if op == "assign" {
                // Operator.assign: track via reductions so GremlinAggregate
                // can detect it. No numeric init needed.
                self.reductions.insert(key.to_string(), (op.clone(), 0.0));
            } else if op != "set" {
                // "set" is tracked via set_keys, not reductions
                let init_val = match val {
                    GremlinValue::Integer(n) => *n as f64,
                    GremlinValue::Float(f) => *f,
                    GremlinValue::Bool(b) if *b => 1.0,
                    _ => 0.0,
                };
                self.reductions
                    .insert(key.to_string(), (op.clone(), init_val));
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
}
