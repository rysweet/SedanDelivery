//! Expression evaluation operators: eval_with_storage, build_filter_ctx,
//! eval_exists_patterns, eval_full_exists_subquery, eval_pattern_comprehension.
//!
//! This module is split across several sibling files for maintainability:
//! * [`function_call`]   — `Expr::FunctionCall` dispatch.
//! * [`element_funcs`]   — element accessor handlers (Property, HasLabel,
//!   Subscript and the per-function helpers `id`, `labels`, `type`,
//!   `properties`, `keys`, `propertiesMatch`, gremlin `T.label/key/value`).
//! * [`path_funcs`]      — path-related helpers (`nodes`, `relationships`,
//!   `length`, `startNode`, `endNode`, gremlin `bothV`/`otherV`).
//! * [`gremlin_value_funcs`] — gremlin `valueMap`, `elementMap`, `values()`,
//!   `properties()`.
//! * [`gremlin_misc_funcs`]  — gremlin `format`, sort, asString and friends.
//! * [`quantifiers`]     — `ListComp`, `Reduce`, and `all/any/none/single`.

use crate::ast::Expr;
use crate::error::GqlError;
use crate::executor::budget::ExecutionBudget;
use crate::executor::filter::eval_expr;
use crate::executor::filter::Row as FilterRow;
use crate::planner::QueryPlanner;
use crate::types::Value;

use super::helpers::{eval_binary_values, value_to_expr};
use super::{ExecContext, MutationStats, QueryExecutor, Row};

mod element_funcs;
mod function_call;
mod gremlin_misc_funcs;
mod gremlin_value_funcs;
mod path_funcs;
mod quantifiers;

#[cfg(test)]
mod tests;

/// Render a [`Value`] as a plain string for `format()`-style placeholders.
pub(super) fn format_value(v: &Value) -> String {
    match v {
        Value::Str(s) => s.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        other => format!("{:?}", other),
    }
}

/// Wrap a property value for `valueMap()` output.
///
/// Single scalar values are wrapped in `List([v])`.
/// Multi-valued vertex properties (stored as `List` of `Map`s with a "value"
/// key) are flattened to `List` of the extracted values.
/// Plain lists (no Maps with "value") are kept as-is.
pub(super) fn valuemap_wrap_property(v: Value) -> Value {
    match v {
        Value::List(items) => {
            if items
                .iter()
                .all(|item| matches!(item, Value::Map(m) if m.contains_key("value")))
                && !items.is_empty()
            {
                // Multi-valued vertex property with meta-properties: extract values
                let values = items
                    .into_iter()
                    .map(|item| match item {
                        Value::Map(m) => m.get("value").cloned().unwrap_or(Value::Null),
                        other => other,
                    })
                    .collect();
                Value::List(values)
            } else {
                // Plain list property — already a list of values
                Value::List(items)
            }
        }
        single => Value::List(vec![single]),
    }
}

impl<'a> QueryExecutor<'a> {
    /// Build a `FilterRow` (borrowed string-key map) from an owned `Row`.
    ///
    /// Pre-allocates the `HashMap` with the exact row size to avoid rehashing.
    pub(crate) fn build_filter_ctx<'r>(
        &'r self,
        _graph: &str,
        row: &'r Row,
    ) -> Result<FilterRow<'r>, GqlError> {
        let mut ctx = FilterRow::with_capacity(row.len() + self.parameters.len());
        for (k, v) in row {
            ctx.insert(k.as_str(), v);
        }
        for (k, v) in &self.parameters {
            ctx.entry(k.as_str()).or_insert(v);
        }
        Ok(ctx)
    }

    /// Evaluate an expression, resolving node/edge property access via the store.
    pub(crate) fn eval_with_storage(
        &self,
        graph: &str,
        expr: &Expr,
        row: &Row,
        ctx: &FilterRow<'_>,
    ) -> Result<Value, GqlError> {
        match expr {
            Expr::Property(obj_expr, key) => self.eval_property(graph, obj_expr, key, row, ctx),
            Expr::Variable(id) => {
                if let Some(v) = row.get(id.0.as_str()) {
                    Ok(v.clone())
                } else if self.gremlin_mode && id.0.starts_with("__gremlin_agg_") {
                    // Aggregate columns may not be carried through intermediate
                    // steps (e.g. .out() after .aggregate()).  Fall back to the
                    // side-effect store which persists across all steps.
                    let se = self.gremlin_side_effects.borrow();
                    if let Some(v) = se.get(id.0.as_str()) {
                        Ok(v.clone())
                    } else {
                        Ok(Value::List(vec![]))
                    }
                } else {
                    eval_expr(expr, ctx)
                }
            }
            Expr::BinaryOp { op, left, right } => {
                let lv = self.eval_with_storage(graph, left, row, ctx)?;
                let rv = self.eval_with_storage(graph, right, row, ctx)?;
                eval_binary_values(*op, lv, rv, ctx)
            }
            Expr::UnaryOp { op, expr: inner } => {
                let v = self.eval_with_storage(graph, inner, row, ctx)?;
                let inner_lit = value_to_expr(&v);
                let rebuilt = Expr::UnaryOp {
                    op: *op,
                    expr: Box::new(inner_lit),
                };
                eval_expr(&rebuilt, ctx)
            }
            Expr::IsNull(inner) => {
                let v = self.eval_with_storage(graph, inner, row, ctx)?;
                Ok(Value::Bool(v.is_null()))
            }
            Expr::IsNotNull(inner) => {
                let v = self.eval_with_storage(graph, inner, row, ctx)?;
                Ok(Value::Bool(!v.is_null()))
            }
            Expr::HasLabel(inner, label) => self.eval_has_label(graph, inner, label, row, ctx),
            Expr::FunctionCall {
                name,
                args,
                distinct,
            } => self.eval_function_call(graph, name, args, *distinct, row, ctx, expr),
            Expr::Case {
                subject,
                alternatives,
                default,
            } => {
                if let Some(subj) = subject {
                    let subj_val = self.eval_with_storage(graph, subj, row, ctx)?;
                    for (when_expr, then_expr) in alternatives {
                        let when_val = self.eval_with_storage(graph, when_expr, row, ctx)?;
                        if subj_val == when_val {
                            return self.eval_with_storage(graph, then_expr, row, ctx);
                        }
                    }
                } else {
                    for (when_expr, then_expr) in alternatives {
                        let when_val = self.eval_with_storage(graph, when_expr, row, ctx)?;
                        if when_val == Value::Bool(true) {
                            return self.eval_with_storage(graph, then_expr, row, ctx);
                        }
                    }
                }
                match default {
                    Some(d) => self.eval_with_storage(graph, d, row, ctx),
                    None => Ok(Value::Null),
                }
            }
            Expr::List(items) => {
                let mut result = Vec::with_capacity(items.len());
                for item in items {
                    result.push(self.eval_with_storage(graph, item, row, ctx)?);
                }
                Ok(Value::List(result))
            }
            Expr::MapLiteral(pairs) => {
                let mut map = indexmap::IndexMap::new();
                for (key, val_expr) in pairs {
                    let val = self.eval_with_storage(graph, val_expr, row, ctx)?;
                    map.insert(key.to_string().into_boxed_str(), val);
                }
                Ok(Value::Map(map))
            }
            Expr::ListComp {
                variable,
                list,
                filter,
                map,
            } => self.eval_list_comp(graph, variable, list, filter.as_deref(), map, row, ctx),
            Expr::Reduce {
                accumulator,
                initial,
                variable,
                list,
                expression,
            } => self.eval_reduce(
                graph,
                accumulator,
                initial,
                variable,
                list,
                expression,
                row,
                ctx,
            ),
            Expr::ExistsSubquery {
                patterns,
                where_clause,
                full_query,
            } => {
                if let Some(inner_query) = full_query {
                    self.eval_full_exists_subquery(graph, inner_query, row)
                } else {
                    self.eval_exists_patterns(graph, patterns, where_clause.as_deref(), row)
                }
            }
            Expr::PatternExpr(patterns) => self.eval_exists_patterns(graph, patterns, None, row),
            Expr::Subscript(obj_expr, idx_expr) => {
                self.eval_subscript(graph, obj_expr, idx_expr, row, ctx)
            }
            _ => eval_expr(expr, ctx),
        }
    }

    pub(crate) fn eval_exists_patterns(
        &self,
        graph: &str,
        patterns: &[crate::ast::PathPattern],
        where_clause: Option<&Expr>,
        outer_row: &Row,
    ) -> Result<Value, GqlError> {
        let planner = QueryPlanner::new(self.store, graph);
        let sub_op = planner.plan_exists_subquery(patterns, where_clause)?;

        let outer = vec![outer_row.clone()];
        let mut budget = ExecutionBudget::default_budget();
        let mut stats = MutationStats::default();
        let mut sub_ctx = ExecContext {
            graph,
            budget: &mut budget,
            outer_rows: &outer,
            stats: &mut stats,
        };
        let sub_rows = self.exec_operator(&mut sub_ctx, &sub_op)?;

        let has_match = sub_rows.iter().any(|sub_row| {
            outer_row
                .iter()
                .filter(|(k, _)| !k.starts_with("__anon"))
                .all(|(k, v)| sub_row.get(k).is_none_or(|sv| sv == v))
        });
        Ok(Value::Bool(has_match))
    }

    pub(crate) fn eval_full_exists_subquery(
        &self,
        graph: &str,
        inner_query: &crate::ast::Query,
        outer_row: &Row,
    ) -> Result<Value, GqlError> {
        let planner =
            QueryPlanner::with_context(self.store, graph, self.registry, &self.parameters);
        let inner_op = planner.plan_unchecked(inner_query)?;
        let outer = vec![outer_row.clone()];
        let mut budget = ExecutionBudget::default_budget();
        let mut stats = MutationStats::default();
        let mut sub_ctx = ExecContext {
            graph,
            budget: &mut budget,
            outer_rows: &outer,
            stats: &mut stats,
        };
        let sub_rows = self.exec_operator(&mut sub_ctx, &inner_op)?;
        let has_match = sub_rows.iter().any(|sub_row| {
            outer_row
                .iter()
                .filter(|(k, _)| !k.starts_with('$') && !k.starts_with("__anon"))
                .all(|(k, v)| sub_row.get(k).is_none_or(|sv| sv == v))
        });
        Ok(Value::Bool(has_match))
    }

    pub(crate) fn eval_pattern_comprehension(
        &self,
        graph: &str,
        patterns: &[crate::ast::PathPattern],
        map_expr: &Expr,
        where_expr: Option<&Expr>,
        outer_row: &Row,
    ) -> Result<Value, GqlError> {
        let planner =
            QueryPlanner::with_context(self.store, graph, self.registry, &self.parameters);
        let sub_op = planner.plan_exists_subquery(patterns, None)?;

        let outer = vec![outer_row.clone()];
        let mut budget = ExecutionBudget::default_budget();
        let mut stats = MutationStats::default();
        let mut sub_ctx = ExecContext {
            graph,
            budget: &mut budget,
            outer_rows: &outer,
            stats: &mut stats,
        };
        let sub_rows = self.exec_operator(&mut sub_ctx, &sub_op)?;

        let mut result = Vec::new();
        for sub_row in &sub_rows {
            let compatible = outer_row
                .iter()
                .filter(|(k, _)| !k.starts_with('$') && !k.starts_with("__anon"))
                .all(|(k, v)| sub_row.get(k.as_str()).is_none_or(|sv| sv == v));
            if !compatible {
                continue;
            }
            if let Some(pred) = where_expr {
                if !matches!(pred, Expr::Bool(true)) {
                    let ctx = self.build_filter_ctx(graph, sub_row)?;
                    match self.eval_with_storage(graph, pred, sub_row, &ctx)? {
                        Value::Bool(true) => {}
                        _ => continue,
                    }
                }
            }
            let ctx = self.build_filter_ctx(graph, sub_row)?;
            result.push(self.eval_with_storage(graph, map_expr, sub_row, &ctx)?);
        }
        Ok(Value::List(result))
    }
}
