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
    ) -> Result<Value, GqlError> {
        if key.is_empty() {
            return Ok(val.clone());
        }
        if let Some(expr) = key.strip_prefix("~math:") {
            return Ok(eval_select_math_expr(expr, val));
        }
        if let Some(rest) = key.strip_prefix("~select_map:") {
            return Ok(self.select_map_fields(rest, val));
        }
        if let Some(rest) = key.strip_prefix("~in_values_min:") {
            if let Some((rel_type, prop)) = rest.split_once(':') {
                return self.resolve_in_values_min(cx, rel_type, prop, val);
            }
        }
        // Handle ~toupper:prop and ~tolower:prop
        if let Some(prop) = key.strip_prefix("~toupper:") {
            let inner = self.apply_path_by_key(cx, prop, val)?;
            return Ok(match inner {
                Value::Str(s) => Value::Str(s.to_uppercase().into_boxed_str()),
                other => other,
            });
        }
        if let Some(prop) = key.strip_prefix("~tolower:") {
            let inner = self.apply_path_by_key(cx, prop, val)?;
            return Ok(match inner {
                Value::Str(s) => Value::Str(s.to_lowercase().into_boxed_str()),
                other => other,
            });
        }
        // Handle ~unfold_values_fold:prop — collect property from list of nodes
        if let Some(prop) = key.strip_prefix("~unfold_values_fold:") {
            if let Value::List(items) = val {
                let mut names: Vec<Value> = Vec::new();
                for item in items {
                    let v = self.apply_path_by_key(cx, prop, item)?;
                    if !matches!(v, Value::Null) {
                        names.push(v);
                    }
                }
                return Ok(Value::List(names));
            }
            return self.apply_path_by_key(cx, prop, val);
        }
        Ok(match key {
            "~label" | "T.label" | "label" => match val {
                Value::Node(nid) => {
                    if let Ok(Some(node)) = self.store.get_node(cx.graph, *nid) {
                        if let Some(label) = node.labels.into_iter().next() {
                            return Ok(Value::Str(label.into_boxed_str()));
                        }
                    }
                    Value::Null
                }
                Value::Edge(eid) => {
                    if let Ok(Some(edge)) = self.store.get_edge(cx.graph, *eid) {
                        return Ok(Value::Str(edge.rel_type.into_boxed_str()));
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
                                return Ok(v.clone());
                            }
                        }
                        Value::Null
                    }
                    Value::Edge(eid) => {
                        if let Ok(Some(edge)) = self.store.get_edge(cx.graph, *eid) {
                            if let Some(v) = edge.properties.get(key) {
                                return Ok(v.clone());
                            }
                        }
                        Value::Null
                    }
                    _ => val.clone(),
                }
            }
        })
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
    ) -> Result<Value, GqlError> {
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
    ) -> Result<Value, GqlError> {
        let Value::Node(node_id) = val else {
            return Ok(Value::Null);
        };

        let rel_types = [rel_type];
        let incoming = self
            .store
            .in_edges_by_type(cx.graph, *node_id, &rel_types)?;
        if incoming.is_empty() {
            return Ok(Value::Null);
        }

        let neighbor_ids: Vec<NodeId> = incoming.iter().map(|entry| entry.neighbor_id).collect();
        let nodes = self.store.get_nodes_batch(cx.graph, &neighbor_ids)?;

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
                return self.resolve_prop_on_value(cx, by_key, selected);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::budget::ExecutionBudget;
    use crate::executor::engine::{ExecContext, MutationStats, QueryExecutor};
    use crate::storage::{GraphStore, InMemoryGraphStore};
    use indexmap::IndexMap;

    fn make_store() -> InMemoryGraphStore {
        let s = InMemoryGraphStore::new();
        s.create_graph("g").unwrap();
        s
    }

    fn alice(s: &InMemoryGraphStore) -> crate::storage::NodeId {
        let mut p = IndexMap::new();
        p.insert("name".into(), Value::Str("Alice".into()));
        p.insert("age".into(), Value::Int(30));
        s.create_node("g", vec!["Person".into()], p).unwrap()
    }

    fn make_cx<'a>(b: &'a mut ExecutionBudget, st: &'a mut MutationStats) -> ExecContext<'a> {
        ExecContext {
            graph: "g",
            budget: b,
            outer_rows: &[],
            stats: st,
        }
    }

    // ── apply_path_by_key ──────────────────────────────────────────

    #[test]
    fn apply_path_empty_key_returns_clone() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        let r = exe.apply_path_by_key(&mut cx, "", &Value::Int(7)).unwrap();
        assert_eq!(r, Value::Int(7));
    }

    #[test]
    fn apply_path_math_marker() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        let r = exe
            .apply_path_by_key(&mut cx, "~math:_+_", &Value::Int(3))
            .unwrap();
        assert_eq!(r, Value::Float(6.0));
    }

    #[test]
    fn apply_path_select_map_marker() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        let mut m: IndexMap<Box<str>, Value> = IndexMap::new();
        m.insert("a".into(), Value::Int(1));
        m.insert("b".into(), Value::Int(2));
        m.insert("c".into(), Value::Int(3));
        let r = exe
            .apply_path_by_key(&mut cx, "~select_map:a,c", &Value::Map(m))
            .unwrap();
        match r {
            Value::Map(m) => {
                assert_eq!(m.len(), 2);
                assert!(m.contains_key("a"));
                assert!(m.contains_key("c"));
                assert!(!m.contains_key("b"));
            }
            other => panic!("expected Map, got {other:?}"),
        }
    }

    #[test]
    fn apply_path_select_map_on_non_map_returns_null() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        let r = exe
            .apply_path_by_key(&mut cx, "~select_map:a", &Value::Int(7))
            .unwrap();
        assert_eq!(r, Value::Null);
    }

    #[test]
    fn apply_path_toupper_on_string() {
        let s = make_store();
        let nid = alice(&s);
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        let r = exe
            .apply_path_by_key(&mut cx, "~toupper:name", &Value::Node(nid))
            .unwrap();
        assert_eq!(r, Value::Str("ALICE".into()));
    }

    #[test]
    fn apply_path_tolower_on_string() {
        let s = make_store();
        let nid = alice(&s);
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        let r = exe
            .apply_path_by_key(&mut cx, "~tolower:name", &Value::Node(nid))
            .unwrap();
        assert_eq!(r, Value::Str("alice".into()));
    }

    #[test]
    fn apply_path_toupper_on_non_string_passthrough() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        // No node with "key" property → inner returns Null which is non-Str
        let r = exe
            .apply_path_by_key(&mut cx, "~toupper:age", &Value::Int(99))
            .unwrap();
        assert_eq!(r, Value::Int(99));
    }

    #[test]
    fn apply_path_unfold_values_fold_on_list() {
        let s = make_store();
        let n1 = alice(&s);
        let mut p = IndexMap::new();
        p.insert("name".into(), Value::Str("Bob".into()));
        let n2 = s.create_node("g", vec!["P".into()], p).unwrap();
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        let list = Value::List(vec![Value::Node(n1), Value::Node(n2)]);
        let r = exe
            .apply_path_by_key(&mut cx, "~unfold_values_fold:name", &list)
            .unwrap();
        match r {
            Value::List(items) => {
                assert_eq!(items.len(), 2);
                assert!(items.contains(&Value::Str("Alice".into())));
                assert!(items.contains(&Value::Str("Bob".into())));
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn apply_path_unfold_values_fold_on_non_list_passthrough() {
        let s = make_store();
        let nid = alice(&s);
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        let r = exe
            .apply_path_by_key(&mut cx, "~unfold_values_fold:name", &Value::Node(nid))
            .unwrap();
        assert_eq!(r, Value::Str("Alice".into()));
    }

    #[test]
    fn apply_path_label_on_node_and_edge() {
        let s = make_store();
        let n = alice(&s);
        let e_src = s
            .create_node("g", vec!["X".into()], IndexMap::new())
            .unwrap();
        let eid = s
            .create_edge("g", n, e_src, "REL".into(), IndexMap::new())
            .unwrap();
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        let r = exe
            .apply_path_by_key(&mut cx, "~label", &Value::Node(n))
            .unwrap();
        assert_eq!(r, Value::Str("Person".into()));
        let r = exe
            .apply_path_by_key(&mut cx, "label", &Value::Node(n))
            .unwrap();
        assert_eq!(r, Value::Str("Person".into()));
        let r = exe
            .apply_path_by_key(&mut cx, "T.label", &Value::Edge(eid))
            .unwrap();
        assert_eq!(r, Value::Str("REL".into()));
    }

    #[test]
    fn apply_path_label_on_unlabelled_node_is_null() {
        let s = make_store();
        let n = s.create_node("g", vec![], IndexMap::new()).unwrap();
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        let r = exe
            .apply_path_by_key(&mut cx, "~label", &Value::Node(n))
            .unwrap();
        assert_eq!(r, Value::Null);
    }

    #[test]
    fn apply_path_label_on_non_element_clones() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        let r = exe
            .apply_path_by_key(&mut cx, "~label", &Value::Int(42))
            .unwrap();
        assert_eq!(r, Value::Int(42));
    }

    #[test]
    fn apply_path_id_on_node_and_edge() {
        let s = make_store();
        let n = alice(&s);
        let n2 = alice(&s);
        let eid = s
            .create_edge("g", n, n2, "K".into(), IndexMap::new())
            .unwrap();
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        assert_eq!(
            exe.apply_path_by_key(&mut cx, "~id", &Value::Node(n))
                .unwrap(),
            Value::Int(n.0 as i64)
        );
        assert_eq!(
            exe.apply_path_by_key(&mut cx, "id", &Value::Edge(eid))
                .unwrap(),
            Value::Int(eid.0 as i64)
        );
        // Non-node/edge passes through.
        assert_eq!(
            exe.apply_path_by_key(&mut cx, "~id", &Value::Str("x".into()))
                .unwrap(),
            Value::Str("x".into())
        );
    }

    #[test]
    fn apply_path_key_value_t_tokens() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        let mut m: IndexMap<Box<str>, Value> = IndexMap::new();
        m.insert("~key".into(), Value::Str("k1".into()));
        m.insert("~value".into(), Value::Int(99));
        let mv = Value::Map(m);
        assert_eq!(
            exe.apply_path_by_key(&mut cx, "~key", &mv).unwrap(),
            Value::Str("k1".into())
        );
        assert_eq!(
            exe.apply_path_by_key(&mut cx, "T.value", &mv).unwrap(),
            Value::Int(99)
        );
        // Map without ~key/~value falls back to clone of the whole map.
        let empty = Value::Map(IndexMap::new());
        assert!(matches!(
            exe.apply_path_by_key(&mut cx, "~key", &empty).unwrap(),
            Value::Map(_)
        ));
    }

    #[test]
    fn apply_path_key_on_non_map_clones() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        assert_eq!(
            exe.apply_path_by_key(&mut cx, "~value", &Value::Int(5))
                .unwrap(),
            Value::Int(5)
        );
    }

    #[test]
    fn apply_path_property_access_on_node() {
        let s = make_store();
        let n = alice(&s);
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        assert_eq!(
            exe.apply_path_by_key(&mut cx, "name", &Value::Node(n))
                .unwrap(),
            Value::Str("Alice".into())
        );
        assert_eq!(
            exe.apply_path_by_key(&mut cx, "missing", &Value::Node(n))
                .unwrap(),
            Value::Null
        );
    }

    #[test]
    fn apply_path_property_access_on_edge() {
        let s = make_store();
        let a = alice(&s);
        let b = alice(&s);
        let mut p = IndexMap::new();
        p.insert("w".into(), Value::Int(7));
        let eid = s.create_edge("g", a, b, "K".into(), p).unwrap();
        let exe = QueryExecutor::new(&s);
        let mut bg = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut bg, &mut st);
        assert_eq!(
            exe.apply_path_by_key(&mut cx, "w", &Value::Edge(eid))
                .unwrap(),
            Value::Int(7)
        );
        assert_eq!(
            exe.apply_path_by_key(&mut cx, "missing", &Value::Edge(eid))
                .unwrap(),
            Value::Null
        );
    }

    #[test]
    fn apply_path_property_on_scalar_passes_through() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        assert_eq!(
            exe.apply_path_by_key(&mut cx, "name", &Value::Str("hi".into()))
                .unwrap(),
            Value::Str("hi".into())
        );
    }

    // ── resolve_prop_on_value_for_filter ───────────────────────────

    #[test]
    fn resolve_filter_empty_name_clones() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let r = exe
            .resolve_prop_on_value_for_filter("g", "", &Value::Int(7))
            .unwrap();
        assert_eq!(r, Value::Int(7));
    }

    #[test]
    fn resolve_filter_select_map_marker() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let mut m: IndexMap<Box<str>, Value> = IndexMap::new();
        m.insert("a".into(), Value::Int(1));
        m.insert("b".into(), Value::Int(2));
        let r = exe
            .resolve_prop_on_value_for_filter("g", "~select_map:a", &Value::Map(m))
            .unwrap();
        match r {
            Value::Map(m) => assert_eq!(m.len(), 1),
            other => panic!("expected Map, got {other:?}"),
        }
    }

    #[test]
    fn resolve_filter_label_t_token_node_and_edge() {
        let s = make_store();
        let n = alice(&s);
        let n2 = alice(&s);
        let eid = s
            .create_edge("g", n, n2, "REL".into(), IndexMap::new())
            .unwrap();
        let exe = QueryExecutor::new(&s);
        let r = exe
            .resolve_prop_on_value_for_filter("g", "~label", &Value::Node(n))
            .unwrap();
        assert_eq!(r, Value::Str("Person".into()));
        let r = exe
            .resolve_prop_on_value_for_filter("g", "T.label", &Value::Edge(eid))
            .unwrap();
        assert_eq!(r, Value::Str("REL".into()));
    }

    #[test]
    fn resolve_filter_label_on_unlabelled_node_is_null() {
        let s = make_store();
        let n = s.create_node("g", vec![], IndexMap::new()).unwrap();
        let exe = QueryExecutor::new(&s);
        let r = exe
            .resolve_prop_on_value_for_filter("g", "~label", &Value::Node(n))
            .unwrap();
        assert_eq!(r, Value::Null);
    }

    #[test]
    fn resolve_filter_id_t_token() {
        let s = make_store();
        let n = alice(&s);
        let exe = QueryExecutor::new(&s);
        let r = exe
            .resolve_prop_on_value_for_filter("g", "~id", &Value::Node(n))
            .unwrap();
        assert_eq!(r, Value::Int(n.0 as i64));
    }

    #[test]
    fn resolve_filter_property_node_and_map() {
        let s = make_store();
        let n = alice(&s);
        let exe = QueryExecutor::new(&s);
        let r = exe
            .resolve_prop_on_value_for_filter("g", "name", &Value::Node(n))
            .unwrap();
        assert_eq!(r, Value::Str("Alice".into()));
        let r = exe
            .resolve_prop_on_value_for_filter("g", "missing", &Value::Node(n))
            .unwrap();
        assert_eq!(r, Value::Null);
        let mut m: IndexMap<Box<str>, Value> = IndexMap::new();
        m.insert("k".into(), Value::Int(7));
        let r = exe
            .resolve_prop_on_value_for_filter("g", "k", &Value::Map(m))
            .unwrap();
        assert_eq!(r, Value::Int(7));
    }

    #[test]
    fn resolve_filter_property_scalar_clones() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let r = exe
            .resolve_prop_on_value_for_filter("g", "x", &Value::Int(5))
            .unwrap();
        assert_eq!(r, Value::Int(5));
    }

    // ── split_prop_accessor ─────────────────────────────────────────

    #[test]
    fn split_prop_accessor_with_map_select() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let r = exe.split_prop_accessor("a__map_select__rest");
        assert_eq!(r, Some(("a", "map_select__rest")));
    }

    #[test]
    fn split_prop_accessor_simple_double_underscore() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let r = exe.split_prop_accessor("alias__field");
        assert_eq!(r, Some(("alias", "field")));
    }

    #[test]
    fn split_prop_accessor_returns_none() {
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        assert_eq!(exe.split_prop_accessor("noseparator"), None);
    }

    // ── resolve_label_alias ─────────────────────────────────────────

    #[test]
    fn resolve_label_alias_matches_path_alias() {
        use crate::ast::Identifier;
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let aliases = vec![Identifier::new("v_2"), Identifier::new("v_1")];
        let row = Row::new();
        let r = exe.resolve_label_alias("v_2", &aliases, &row);
        assert_eq!(r, Some("v_2".to_string()));
        let r = exe.resolve_label_alias("missing", &aliases, &row);
        assert_eq!(r, None);
    }

    // ── Bug #1194: resolve_in_values_min must propagate storage errors ─────
    //
    // Currently resolve_in_values_min and apply_path_by_key return Value
    // and silently convert storage errors to Value::Null via
    // `Err(_) => return Value::Null`. After fix: change signatures to
    // return Result<Value, GqlError> and propagate errors with `?`.

    #[test]
    fn apply_path_by_key_in_values_min_propagates_storage_error() {
        // Use a nonexistent graph name to trigger a storage error.
        let s = make_store();
        let nid = alice(&s);
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = ExecContext {
            graph: "nonexistent_graph",
            budget: &mut b,
            outer_rows: &[],
            stats: &mut st,
        };
        // ~in_values_min:<relType>:<prop> triggers resolve_in_values_min
        let result = exe.apply_path_by_key(&mut cx, "~in_values_min:KNOWS:name", &Value::Node(nid));
        // After fix: should be Err (storage error), not Ok(Value::Null)
        assert!(
            result.is_err(),
            "storage error should propagate from apply_path_by_key, got: {result:?}"
        );
    }

    #[test]
    fn apply_path_by_key_returns_ok_for_valid_access() {
        // Regression: valid property access should still work.
        let s = make_store();
        let nid = alice(&s);
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        let result = exe.apply_path_by_key(&mut cx, "name", &Value::Node(nid));
        // After fix: should be Ok(Value::Str("Alice"))
        assert!(
            result.is_ok(),
            "valid access should return Ok, got: {result:?}"
        );
        assert_eq!(result.unwrap(), Value::Str("Alice".into()));
    }

    #[test]
    fn apply_path_by_key_empty_key_returns_ok_clone() {
        // Regression: empty key should return Ok with cloned value.
        let s = make_store();
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = make_cx(&mut b, &mut st);
        let result = exe.apply_path_by_key(&mut cx, "", &Value::Int(7));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Value::Int(7));
    }

    #[test]
    fn resolve_prop_on_value_propagates_error() {
        // resolve_prop_on_value delegates to apply_path_by_key,
        // so it should also return Result<Value, GqlError>.
        let s = make_store();
        let nid = alice(&s);
        let exe = QueryExecutor::new(&s);
        let mut b = ExecutionBudget::unlimited();
        let mut st = MutationStats::default();
        let mut cx = ExecContext {
            graph: "nonexistent_graph",
            budget: &mut b,
            outer_rows: &[],
            stats: &mut st,
        };
        let result =
            exe.resolve_prop_on_value(&mut cx, "~in_values_min:KNOWS:name", &Value::Node(nid));
        assert!(
            result.is_err(),
            "storage error should propagate from resolve_prop_on_value, got: {result:?}"
        );
    }
}
