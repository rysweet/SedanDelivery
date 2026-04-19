//! `by` step variant: by() after order() — primary sort.
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
    pub(super) fn handle_by_order(
        &mut self,
        key: &str,
        descending: bool,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
        Ok(())
    }
}
