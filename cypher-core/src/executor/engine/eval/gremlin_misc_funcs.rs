//! Miscellaneous Gremlin function handlers split out of `eval`:
//! `format`, `format` with by-modulators, `sort` (graph-aware), `asString`,
//! and the `Column.{keys,values}` / unfold map-entry helpers.

use crate::error::GqlError;
use crate::executor::filter::eval_function_with_vals;
use crate::types::Value;

use super::super::QueryExecutor;
use super::format_value;

impl<'a> QueryExecutor<'a> {
    pub(super) fn eval_func_gremlin_format(
        &self,
        graph: &str,
        name: &smol_str::SmolStr,
        evaled_args: Vec<Value>,
    ) -> Result<Value, GqlError> {
        // format(): resolve %{key} from node/edge properties and/or as-labels.
        // Priority: node/edge properties first, then as-labels for remaining.
        // Returns Null if any %{key} placeholder can't be resolved.

        let labels_map = match evaled_args.get(2) {
            Some(Value::Map(m)) => Some(m.clone()),
            _ => None,
        };

        match (evaled_args.first(), evaled_args.get(1)) {
            (Some(Value::Node(node_id)), Some(Value::Str(pattern))) => {
                let node_id = *node_id;
                let pattern = pattern.clone();
                match self.cached_get_node(graph, node_id)? {
                    Some(data) => {
                        let mut result = pattern.to_string();
                        // Resolve from node properties first
                        for (k, v) in data.properties.iter() {
                            let placeholder = format!("%{{{}}}", k);
                            let val_str = format_value(v);
                            result = result.replace(&placeholder, &val_str);
                        }
                        // Then resolve remaining from labels
                        if result.contains("%{") {
                            if let Some(ref labels) = labels_map {
                                for (k, v) in labels.iter() {
                                    let placeholder = format!("%{{{}}}", k);
                                    if result.contains(&placeholder) {
                                        let val_str = format_value(v);
                                        result = result.replace(&placeholder, &val_str);
                                    }
                                }
                            }
                        }
                        if result.contains("%{") {
                            Ok(Value::Null)
                        } else {
                            Ok(Value::Str(result.into()))
                        }
                    }
                    None => Ok(Value::Null),
                }
            }
            (Some(Value::Edge(edge_id)), Some(Value::Str(pattern))) => {
                let edge_id = *edge_id;
                let pattern = pattern.clone();
                match self.cached_get_edge(graph, edge_id)? {
                    Some(data) => {
                        let mut result = pattern.to_string();
                        for (k, v) in data.properties.iter() {
                            let placeholder = format!("%{{{}}}", k);
                            let val_str = format_value(v);
                            result = result.replace(&placeholder, &val_str);
                        }
                        if result.contains("%{") {
                            if let Some(ref labels) = labels_map {
                                for (k, v) in labels.iter() {
                                    let placeholder = format!("%{{{}}}", k);
                                    if result.contains(&placeholder) {
                                        let val_str = format_value(v);
                                        result = result.replace(&placeholder, &val_str);
                                    }
                                }
                            }
                        }
                        if result.contains("%{") {
                            Ok(Value::Null)
                        } else {
                            Ok(Value::Str(result.into()))
                        }
                    }
                    None => Ok(Value::Null),
                }
            }
            (_, Some(Value::Str(pattern))) if labels_map.is_some() => {
                // Non-node/edge: resolve from labels only
                let mut result = pattern.to_string();
                if let Some(ref labels) = labels_map {
                    for (k, v) in labels.iter() {
                        let placeholder = format!("%{{{}}}", k);
                        if result.contains(&placeholder) {
                            let val_str = format_value(v);
                            result = result.replace(&placeholder, &val_str);
                        }
                    }
                }
                if result.contains("%{") {
                    Ok(Value::Null)
                } else {
                    Ok(Value::Str(result.to_string().into()))
                }
            }
            (Some(Value::Str(_)), Some(Value::Str(pattern))) => {
                if !pattern.contains("%{") {
                    Ok(Value::Str(pattern.clone()))
                } else {
                    eval_function_with_vals(name, evaled_args)
                }
            }
            _ => eval_function_with_vals(name, evaled_args),
        }
    }

    pub(super) fn eval_func_gremlin_format_by(
        &self,
        graph: &str,
        evaled_args: Vec<Value>,
    ) -> Result<Value, GqlError> {
        // format() with by() modulations + property resolution.
        // args = [traverser, pattern, by1, by2, ...]
        // First replace %{_} with by-values, then resolve %{key} from
        // node/edge properties.
        if evaled_args.len() < 2 {
            return Ok(Value::Null);
        }
        if matches!(evaled_args.first(), Some(Value::Null)) {
            return Ok(Value::Null);
        }
        let pattern = match evaled_args.get(1) {
            Some(Value::Str(s)) => s.to_string(),
            _ => return Ok(Value::Null),
        };
        let by_values = &evaled_args[2..];
        let mut result = pattern;
        // Replace %{_} with by-values cyclically
        let mut by_idx = 0;
        while let Some(pos) = result.find("%{_}") {
            if by_values.is_empty() {
                break;
            }
            let val = &by_values[by_idx % by_values.len()];
            let val_str = format_value(val);
            result = format!("{}{}{}", &result[..pos], val_str, &result[pos + 4..]);
            by_idx += 1;
        }
        // Resolve remaining %{key} from node/edge properties
        if result.contains("%{") {
            match evaled_args.first() {
                Some(Value::Node(node_id)) => {
                    if let Some(data) = self.cached_get_node(graph, *node_id)? {
                        for (k, v) in data.properties.iter() {
                            let placeholder = format!("%{{{}}}", k);
                            let val_str = format_value(v);
                            result = result.replace(&placeholder, &val_str);
                        }
                    }
                }
                Some(Value::Edge(edge_id)) => {
                    if let Some(data) = self.cached_get_edge(graph, *edge_id)? {
                        for (k, v) in data.properties.iter() {
                            let placeholder = format!("%{{{}}}", k);
                            let val_str = format_value(v);
                            result = result.replace(&placeholder, &val_str);
                        }
                    }
                }
                _ => {}
            }
        }
        if result.contains("%{") {
            Ok(Value::Null)
        } else {
            Ok(Value::Str(result.into()))
        }
    }

    pub(super) fn eval_func_gremlin_sort_local(
        &self,
        graph: &str,
        name: &smol_str::SmolStr,
        evaled_args: Vec<Value>,
    ) -> Result<Value, GqlError> {
        // Graph-aware sort for lists containing nodes/edges.
        // For pure value lists (ints, floats, strings), delegate to
        // the filter version to preserve existing behavior.
        let needs_graph = match evaled_args.first() {
            Some(Value::List(items)) => items
                .iter()
                .any(|v| matches!(v, Value::Node(_) | Value::Edge(_))),
            _ => false,
        };
        if !needs_graph {
            eval_function_with_vals(name, evaled_args)
        } else {
            let mut vals_iter = evaled_args.into_iter();
            let list_val = vals_iter.next().unwrap_or(Value::Null);
            let by_key = match vals_iter.next() {
                Some(Value::Str(s)) if !s.is_empty() => Some(s),
                _ => None,
            };
            let descending = matches!(
                vals_iter.next(),
                Some(Value::Str(s)) if s.as_ref() == "desc"
            );
            match list_val {
                Value::List(mut items) => {
                    if let Some(ref key) = by_key {
                        items.sort_by(|a, b| {
                            let a_prop = self.extract_sort_key_with_graph(graph, a, key);
                            let b_prop = self.extract_sort_key_with_graph(graph, b, key);
                            let ord = crate::executor::engine::helpers::gremlin_cmp_values(
                                &a_prop, &b_prop,
                            );
                            if descending {
                                ord.reverse()
                            } else {
                                ord
                            }
                        });
                        // Remove items lacking the sort property
                        items.retain(|item| {
                            !matches!(
                                self.extract_sort_key_with_graph(graph, item, key,),
                                Value::Null
                            )
                        });
                    } else {
                        items.sort_by(|a, b| {
                            let ord = crate::executor::engine::helpers::gremlin_cmp_values(a, b);
                            if descending {
                                ord.reverse()
                            } else {
                                ord
                            }
                        });
                    }
                    Ok(Value::List(items))
                }
                other => Ok(other),
            }
        }
    }

    pub(super) fn eval_func_gremlin_asstring(
        &self,
        graph: &str,
        name: &smol_str::SmolStr,
        evaled_args: Vec<Value>,
    ) -> Result<Value, GqlError> {
        if let Some(Value::Edge(eid)) = evaled_args.first() {
            if let Ok(Some(edge)) = self.store.get_edge(graph, *eid) {
                return Ok(Value::Str(
                    format!(
                        "e[{}][{}-{}->{}]",
                        eid.0, edge.src.0, edge.rel_type, edge.dst.0
                    )
                    .into(),
                ));
            }
        }
        eval_function_with_vals(name, evaled_args)
    }
}
