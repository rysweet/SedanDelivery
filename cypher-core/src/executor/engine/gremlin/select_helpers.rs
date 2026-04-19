//! Gremlin executor: `select_helpers` step implementations.

use indexmap::IndexMap;

use crate::ast::{Expr, Identifier};
use crate::error::GqlError;
use crate::planner::GremlinSelectTraversal;
use crate::storage::NodeId;
use crate::types::Value;

use crate::executor::engine::helpers::gremlin_cmp_values;
use crate::executor::engine::{ExecContext, QueryExecutor, Row};

#[allow(unused_imports)]
use super::helpers::*;

impl<'a> QueryExecutor<'a> {
    /// Resolve a label (now pre-resolved at compile time to an internal
    /// alias like "v_2") against path_aliases.  Falls back to the old
    /// row-based resolution for backward compatibility.
    pub(super) fn resolve_label_alias(
        &self,
        label: &str,
        path_aliases: &[Identifier],
        _row: &Row,
    ) -> Option<String> {
        // Direct match: the label is an internal alias in path_aliases.
        // This is the primary path after compile-time resolution.
        if path_aliases.iter().any(|a| a.as_str() == label) {
            return Some(label.to_string());
        }
        None
    }

    /// Apply a single by-key transformation to a path element value.
    pub(super) fn apply_path_by_key(
        &self,
        cx: &mut ExecContext<'_>,
        key: &str,
        val: &Value,
    ) -> Value {
        if key.is_empty() {
            return val.clone();
        }
        if let Some(expr) = key.strip_prefix("~math:") {
            return eval_select_math_expr(expr, val);
        }
        if let Some(rest) = key.strip_prefix("~select_map:") {
            return self.select_map_fields(rest, val);
        }
        if let Some(rest) = key.strip_prefix("~in_values_min:") {
            if let Some((rel_type, prop)) = rest.split_once(':') {
                return self.resolve_in_values_min(cx, rel_type, prop, val);
            }
        }
        // Handle ~toupper:prop and ~tolower:prop
        if let Some(prop) = key.strip_prefix("~toupper:") {
            let inner = self.apply_path_by_key(cx, prop, val);
            return match inner {
                Value::Str(s) => Value::Str(s.to_uppercase().into_boxed_str()),
                other => other,
            };
        }
        if let Some(prop) = key.strip_prefix("~tolower:") {
            let inner = self.apply_path_by_key(cx, prop, val);
            return match inner {
                Value::Str(s) => Value::Str(s.to_lowercase().into_boxed_str()),
                other => other,
            };
        }
        // Handle ~unfold_values_fold:prop — collect property from list of nodes
        if let Some(prop) = key.strip_prefix("~unfold_values_fold:") {
            if let Value::List(items) = val {
                let names: Vec<Value> = items
                    .iter()
                    .filter_map(|item| {
                        let v = self.apply_path_by_key(cx, prop, item);
                        if matches!(v, Value::Null) {
                            None
                        } else {
                            Some(v)
                        }
                    })
                    .collect();
                return Value::List(names);
            }
            return self.apply_path_by_key(cx, prop, val);
        }
        match key {
            "~label" | "T.label" | "label" => match val {
                Value::Node(nid) => {
                    if let Ok(Some(node)) = self.store.get_node(cx.graph, *nid) {
                        if let Some(label) = node.labels.into_iter().next() {
                            return Value::Str(label.into_boxed_str());
                        }
                    }
                    Value::Null
                }
                Value::Edge(eid) => {
                    if let Ok(Some(edge)) = self.store.get_edge(cx.graph, *eid) {
                        return Value::Str(edge.rel_type.into_boxed_str());
                    }
                    Value::Null
                }
                _ => val.clone(),
            },
            "~id" | "T.id" | "id" => match val {
                Value::Node(nid) => Value::Int(nid.0 as i64),
                Value::Edge(eid) => Value::Int(eid.0 as i64),
                _ => val.clone(),
            },
            "~key" | "T.key" | "key" => match val {
                Value::Map(m) => m.get("~key").cloned().unwrap_or_else(|| val.clone()),
                _ => val.clone(),
            },
            "~value" | "T.value" | "value" => match val {
                Value::Map(m) => m.get("~value").cloned().unwrap_or_else(|| val.clone()),
                _ => val.clone(),
            },
            _ => {
                // Property access
                match val {
                    Value::Node(nid) => {
                        if let Ok(Some(node)) = self.store.get_node(cx.graph, *nid) {
                            if let Some(v) = node.properties.get(key) {
                                return v.clone();
                            }
                        }
                        Value::Null
                    }
                    Value::Edge(eid) => {
                        if let Ok(Some(edge)) = self.store.get_edge(cx.graph, *eid) {
                            if let Some(v) = edge.properties.get(key) {
                                return v.clone();
                            }
                        }
                        Value::Null
                    }
                    _ => val.clone(),
                }
            }
        }
    }

    /// Execute `GremlinMatch` — declarative pattern matching with named bindings.
    ///
    /// Resolve a property on a Value (Node or Edge). Used by select()
    /// by-modulation to apply property extraction to Pop-resolved values.
    pub(super) fn resolve_prop_on_value(
        &self,
        cx: &mut ExecContext<'_>,
        prop_name: &str,
        val: &Value,
    ) -> Value {
        self.apply_path_by_key(cx, prop_name, val)
    }

    pub(crate) fn resolve_prop_on_value_for_filter(
        &self,
        graph: &str,
        prop_name: &str,
        val: &Value,
    ) -> Result<Value, GqlError> {
        if prop_name.is_empty() {
            return Ok(val.clone());
        }
        if let Some(rest) = prop_name.strip_prefix("~select_map:") {
            return Ok(self.select_map_fields(rest, val));
        }
        if let Some(rest) = prop_name.strip_prefix("~in_values_min:") {
            if let Some((rel_type, prop)) = rest.split_once(':') {
                return self.resolve_in_values_min_for_filter(graph, rel_type, prop, val);
            }
        }
        // Only the canonical Gremlin T-token forms (`~label`, `T.label`, etc.)
        // are intercepted here. The bare lowercase aliases (`"label"`, `"id"`,
        // `"key"`, `"value"`) are NOT special-cased: in the filter context the
        // key passed in is always a literal property name, so treating
        // `has("value", X)` as a T-token would shadow legitimate user
        // properties named `value`/`id`/`label`/`key` and silently return the
        // element itself — see issue #1710.
        match prop_name {
            "~label" | "T.label" => match val {
                Value::Node(node_id) => Ok(match self.cached_get_node(graph, *node_id)? {
                    Some(node_data) => node_data
                        .labels
                        .into_iter()
                        .next()
                        .map(|label| Value::Str(label.into_boxed_str()))
                        .unwrap_or(Value::Null),
                    None => Value::Null,
                }),
                Value::Edge(edge_id) => Ok(match self.cached_get_edge(graph, *edge_id)? {
                    Some(edge_data) => Value::Str(edge_data.rel_type.into_boxed_str()),
                    None => Value::Null,
                }),
                _ => Ok(val.clone()),
            },
            "~id" | "T.id" => Ok(match val {
                Value::Node(nid) => Value::Int(nid.0 as i64),
                Value::Edge(eid) => Value::Int(eid.0 as i64),
                _ => val.clone(),
            }),
            "~key" | "T.key" => Ok(match val {
                Value::Map(m) => m.get("~key").cloned().unwrap_or_else(|| val.clone()),
                _ => val.clone(),
            }),
            "~value" | "T.value" => Ok(match val {
                Value::Map(m) => m.get("~value").cloned().unwrap_or_else(|| val.clone()),
                _ => val.clone(),
            }),
            _ => match val {
                Value::Node(node_id) => Ok(match self.cached_get_node(graph, *node_id)? {
                    Some(node_data) => node_data
                        .properties
                        .get(prop_name)
                        .cloned()
                        .unwrap_or(Value::Null),
                    None => Value::Null,
                }),
                Value::Edge(edge_id) => Ok(match self.cached_get_edge(graph, *edge_id)? {
                    Some(edge_data) => edge_data
                        .properties
                        .get(prop_name)
                        .cloned()
                        .unwrap_or(Value::Null),
                    None => Value::Null,
                }),
                Value::Map(m) => Ok(m.get(prop_name).cloned().unwrap_or(Value::Null)),
                _ => Ok(val.clone()),
            },
        }
    }

    fn resolve_in_values_min(
        &self,
        cx: &mut ExecContext<'_>,
        rel_type: &str,
        prop: &str,
        val: &Value,
    ) -> Value {
        let Value::Node(node_id) = val else {
            return Value::Null;
        };

        let rel_types = [rel_type];
        let incoming = match self.store.in_edges_by_type(cx.graph, *node_id, &rel_types) {
            Ok(entries) => entries,
            Err(_) => return Value::Null,
        };
        if incoming.is_empty() {
            return Value::Null;
        }

        let neighbor_ids: Vec<NodeId> = incoming.iter().map(|entry| entry.neighbor_id).collect();
        let nodes = match self.store.get_nodes_batch(cx.graph, &neighbor_ids) {
            Ok(nodes) => nodes,
            Err(_) => return Value::Null,
        };

        let mut best: Option<Value> = None;
        for node in nodes.into_iter().flatten() {
            let Some(candidate) = node.properties.get(prop).cloned() else {
                continue;
            };
            match &best {
                Some(current) if gremlin_cmp_values(&candidate, current).is_ge() => {}
                _ => best = Some(candidate),
            }
        }

        best.unwrap_or(Value::Null)
    }

    fn resolve_in_values_min_for_filter(
        &self,
        graph: &str,
        rel_type: &str,
        prop: &str,
        val: &Value,
    ) -> Result<Value, GqlError> {
        let Value::Node(node_id) = val else {
            return Ok(Value::Null);
        };

        let rel_types = [rel_type];
        let incoming = match self.store.in_edges_by_type(graph, *node_id, &rel_types) {
            Ok(entries) => entries,
            Err(_) => return Ok(Value::Null),
        };
        if incoming.is_empty() {
            return Ok(Value::Null);
        }

        let neighbor_ids: Vec<NodeId> = incoming.iter().map(|entry| entry.neighbor_id).collect();
        let nodes = match self.store.get_nodes_batch(graph, &neighbor_ids) {
            Ok(nodes) => nodes,
            Err(_) => return Ok(Value::Null),
        };

        let mut best: Option<Value> = None;
        for node in nodes.into_iter().flatten() {
            let Some(candidate) = node.properties.get(prop).cloned() else {
                continue;
            };
            match &best {
                Some(current) if gremlin_cmp_values(&candidate, current).is_ge() => {}
                _ => best = Some(candidate),
            }
        }

        Ok(best.unwrap_or(Value::Null))
    }

    fn select_map_fields(&self, fields: &str, val: &Value) -> Value {
        let Value::Map(map) = val else {
            return Value::Null;
        };
        let mut selected = IndexMap::new();
        for field in fields.split(',') {
            if let Some(value) = map.get(field) {
                selected.insert(field.to_string().into_boxed_str(), value.clone());
            }
        }
        Value::Map(selected)
    }

    pub(super) fn resolve_map_select_value(&self, row: &Row, encoded: &str) -> Option<Value> {
        let rest = encoded.strip_prefix("__map_select__")?;
        let (map_alias, key_alias) = rest.split_once("__keyalias__")?;
        let map_val = row.get(map_alias)?;
        let key_val = row.get(key_alias)?;
        let Value::Map(map) = map_val else {
            return None;
        };
        let key = value_to_map_key(key_val);
        map.get(key.as_ref()).cloned()
    }

    pub(super) fn split_prop_accessor<'b>(&self, rest: &'b str) -> Option<(&'b str, &'b str)> {
        if let Some(map_pos) = rest.find("__map_select__") {
            if map_pos >= 2 {
                return Some((&rest[..map_pos - 2], &rest[map_pos..]));
            }
        }
        let sep_pos = rest.find("__")?;
        Some((&rest[..sep_pos], &rest[sep_pos + 2..]))
    }

    pub(super) fn apply_select_by_traversal(
        &self,
        cx: &mut ExecContext<'_>,
        row: &Row,
        src_var: &Identifier,
        selected: Value,
        traversal: &GremlinSelectTraversal,
    ) -> Result<Value, GqlError> {
        let GremlinSelectTraversal::Traversal { op, out_alias } = traversal;
        let mut seed = row.clone();
        seed.insert(src_var.0.to_string(), selected);
        self.foreach_seed.borrow_mut().replace(vec![seed]);
        let sub_rows = self.exec_operator(cx, op)?;
        self.foreach_seed.borrow_mut().take();
        Ok(sub_rows
            .first()
            .and_then(|sub_row| {
                sub_row
                    .get(out_alias.as_str())
                    .or_else(|| sub_row.get(src_var.0.as_str()))
                    .cloned()
            })
            .unwrap_or(Value::Null))
    }

    pub(super) fn eval_gremlin_by_key(
        &self,
        cx: &mut ExecContext<'_>,
        row: &Row,
        value_alias: &Identifier,
        by_key: &str,
    ) -> Result<Value, GqlError> {
        if let Some(selected) = row.get(value_alias.as_str()) {
            if by_key.starts_with('~') {
                return Ok(self.resolve_prop_on_value(cx, by_key, selected));
            }
        }
        let ctx = self.build_filter_ctx(cx.graph, row)?;
        let prop_expr = Expr::Property(
            Box::new(Expr::Variable(value_alias.clone())),
            smol_str::SmolStr::new(by_key),
        );
        Ok(self
            .eval_with_storage(cx.graph, &prop_expr, row, &ctx)
            .unwrap_or(Value::Null))
    }
}
