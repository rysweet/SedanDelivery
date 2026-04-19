//! `Expr::FunctionCall` dispatch split out of `eval`.
//!
//! Per-function bodies live in `element_funcs`, `path_funcs`,
//! `gremlin_value_funcs`, and `gremlin_misc_funcs`. This file just routes by
//! lower-cased name. Special-cased arms (`all/any/none/single`, `exists`,
//! `__patterncomp`) bypass standard argument pre-evaluation.

use crate::ast::Expr;
use crate::error::GqlError;
use crate::executor::filter::{
    eval_expr, eval_function_with_vals, map_key_to_value, Row as FilterRow,
};
use crate::types::Value;

use super::super::QueryExecutor;
use super::super::Row;

impl<'a> QueryExecutor<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn eval_function_call(
        &self,
        graph: &str,
        name: &smol_str::SmolStr,
        args: &[Expr],
        distinct: bool,
        row: &Row,
        ctx: &FilterRow<'_>,
        full_expr: &Expr,
    ) -> Result<Value, GqlError> {
        // Special arms — these inspect args before evaluation or have
        // unique short-circuit semantics.
        match name.to_lowercase().as_str() {
            "all" | "any" | "none" | "single" => {
                return self.eval_quantifier_func(graph, name, args, row, ctx, full_expr);
            }
            "exists" => {
                if args.len() == 1 {
                    let val = self.eval_with_storage(graph, &args[0], row, ctx)?;
                    return Ok(Value::Bool(!val.is_null()));
                }
                return eval_expr(full_expr, ctx);
            }
            "__patterncomp" => {
                if args.len() >= 2 {
                    if let Expr::PatternExpr(patterns) = &args[0] {
                        let map_expr = &args[1];
                        let where_expr = args.get(2);
                        return self.eval_pattern_comprehension(
                            graph, patterns, map_expr, where_expr, row,
                        );
                    }
                }
                debug_assert!(false, "__patterncomp: malformed arguments");
                return Ok(Value::List(vec![]));
            }
            _ => {}
        }

        let evaled_args: Vec<Value> = args
            .iter()
            .map(|a| self.eval_with_storage(graph, a, row, ctx))
            .collect::<Result<Vec<_>, _>>()?;

        match name.to_lowercase().as_str() {
            "haslabel" => self.eval_func_has_label(graph, &evaled_args),
            "propertiesmatch" => self.eval_func_properties_match(graph, &evaled_args),
            "labels" => self.eval_func_labels(graph, &evaled_args),
            "gremlin_single_label" => self.eval_func_gremlin_single_label(graph, &evaled_args),
            "gremlin_property_key" => self.eval_func_gremlin_property_key(&evaled_args),
            "gremlin_property_value" => self.eval_func_gremlin_property_value(&evaled_args),
            "type" => self.eval_func_type(graph, &evaled_args),
            "id" => self.eval_func_id(&evaled_args),
            "properties" => self.eval_func_properties(graph, &evaled_args),
            "keys" => self.eval_func_keys(graph, &evaled_args),
            "nodes" => self.eval_func_nodes(&evaled_args),
            "relationships" => self.eval_func_relationships(&evaled_args),
            "length" => self.eval_func_length(name, &evaled_args, distinct, ctx),
            "startnode" => self.eval_func_startnode(graph, &evaled_args),
            "endnode" => self.eval_func_endnode(graph, &evaled_args),
            "gremlin_both_v" => self.eval_func_gremlin_both_v(graph, evaled_args),
            "gremlin_other_v" => self.eval_func_gremlin_other_v(graph, evaled_args),
            "gremlin_valuemap" => self.eval_func_gremlin_valuemap(graph, evaled_args),
            "gremlin_elementmap" => self.eval_func_gremlin_elementmap(graph, evaled_args),
            "list_compact" => Ok(Value::List(
                evaled_args
                    .into_iter()
                    .filter(|v| !matches!(v, Value::Null))
                    .collect(),
            )),
            "gremlin_values_all" => self.eval_func_gremlin_values_all(graph, evaled_args),
            "gremlin_values_keys" => self.eval_func_gremlin_values_keys(graph, evaled_args),
            "gremlin_properties_entries" => {
                self.eval_func_gremlin_properties_entries(graph, evaled_args)
            }
            "gremlin_format" => self.eval_func_gremlin_format(graph, name, evaled_args),
            "gremlin_format_by" => self.eval_func_gremlin_format_by(graph, evaled_args),
            "gremlin_sort_local" => self.eval_func_gremlin_sort_local(graph, name, evaled_args),
            "gremlin_asstring" => self.eval_func_gremlin_asstring(graph, name, evaled_args),
            "gremlin_column_key" => match evaled_args.into_iter().next() {
                Some(Value::Map(m)) if m.len() == 1 => {
                    Ok(map_key_to_value(m.keys().next().unwrap()))
                }
                Some(other) => eval_function_with_vals("keys", vec![other]),
                None => Ok(Value::Null),
            },
            "gremlin_select_values" => match evaled_args.into_iter().next() {
                Some(Value::Map(m)) if m.len() == 1 => {
                    let v = m.values().next().cloned().unwrap_or(Value::Null);
                    match v {
                        Value::List(_) => Ok(Value::List(vec![v])),
                        other => Ok(other),
                    }
                }
                Some(Value::Map(m)) => Ok(Value::List(m.values().cloned().collect())),
                Some(Value::List(items)) => Ok(Value::List(items)),
                Some(other) => Ok(other),
                None => Ok(Value::Null),
            },
            "gremlin_column_value" => match evaled_args.into_iter().next() {
                Some(Value::Map(m)) if m.len() == 1 => {
                    Ok(m.values().next().cloned().unwrap_or(Value::Null))
                }
                Some(other) => eval_function_with_vals("values", vec![other]),
                None => Ok(Value::Null),
            },
            "gremlin_unfold_map_entry_key" => match evaled_args.into_iter().next() {
                Some(Value::Map(m)) if m.len() == 1 => {
                    Ok(map_key_to_value(m.keys().next().unwrap()))
                }
                Some(other) => eval_function_with_vals("keys", vec![other]),
                None => Ok(Value::Null),
            },
            "gremlin_unfold_map_entry_value" => match evaled_args.into_iter().next() {
                Some(Value::Map(m)) if m.len() == 1 => {
                    Ok(m.values().next().cloned().unwrap_or(Value::Null))
                }
                Some(other) => eval_function_with_vals("values", vec![other]),
                None => Ok(Value::Null),
            },
            _ => eval_function_with_vals(name, evaled_args),
        }
    }
}
