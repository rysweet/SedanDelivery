//! SET and REMOVE operator implementations.

use indexmap::IndexMap;

use crate::error::{ExecutionError, GqlError};
use crate::planner::{Operator, RemoveItem, SetItem};
use crate::types::Value;

use crate::executor::engine::helpers::{extract_node_id, undefined_var};
use crate::executor::engine::{ExecContext, MutationStats, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_set(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Set { items, child } = op else {
            debug_assert!(false, "expected Set operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Set operator".into(),
            }));
        };
        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        for row in input {
            cx.budget.check_or_err()?;
            let current_row = row;
            for item in items {
                self.apply_set_item(cx.graph, &current_row, item, cx.stats)?;
            }
            cx.budget.record_row();
            out.push(current_row);
        }
        self.invalidate_property_cache();
        Ok(out)
    }

    pub(crate) fn exec_remove(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Remove { items, child } = op else {
            debug_assert!(false, "expected Remove operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Remove operator".into(),
            }));
        };
        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        for row in input {
            cx.budget.check_or_err()?;
            for item in items {
                match item {
                    RemoveItem::Property { alias, key } => {
                        let val = row
                            .get(alias.0.as_str())
                            .ok_or_else(|| undefined_var(&alias.0))?;
                        match val {
                            Value::Null => {
                                // REMOVE on null (from OPTIONAL MATCH) is a no-op
                            }
                            Value::Node(node_id) => {
                                self.store.remove_node_property(cx.graph, *node_id, key)?;
                            }
                            Value::Edge(edge_id) => {
                                self.store.remove_edge_property(cx.graph, *edge_id, key)?;
                            }
                            other => {
                                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                    expected: "Node or Relationship".to_string(),
                                    actual: other.type_name().to_string(),
                                }));
                            }
                        }
                        // REMOVE property does not count as properties_set.
                    }
                    RemoveItem::Labels { alias, labels } => {
                        let val = row
                            .get(alias.0.as_str())
                            .ok_or_else(|| undefined_var(&alias.0))?;
                        // REMOVE labels on null (from OPTIONAL MATCH) is a no-op
                        if val.is_null() {
                            continue;
                        }
                        let node_id = extract_node_id(val, &alias.0)?;
                        let label_strings: Vec<String> =
                            labels.iter().map(|l| l.as_str().to_string()).collect();
                        let removed_count = label_strings.len() as u64;
                        self.store
                            .remove_node_labels(cx.graph, node_id, &label_strings)?;
                        cx.stats.labels_removed += removed_count;
                    }
                }
            }
            cx.budget.record_row();
            out.push(row);
        }
        self.invalidate_property_cache();
        Ok(out)
    }

    /// Apply a single `SetItem` (SET n.prop = val, SET n = {map}, SET n += {map}, SET n:Label).
    /// Handles both Node and Edge targets for property operations.
    pub(crate) fn apply_set_item(
        &self,
        graph: &str,
        row: &Row,
        item: &SetItem,
        stats: &mut MutationStats,
    ) -> Result<(), GqlError> {
        match item {
            SetItem::Property { alias, key, value } => {
                let val = row
                    .get(alias.0.as_str())
                    .ok_or_else(|| undefined_var(&alias.0))?;
                // SET on a null variable (from OPTIONAL MATCH) is silently ignored.
                if val.is_null() {
                    return Ok(());
                }
                let ctx = self.build_filter_ctx(graph, row)?;
                let prop_val = self.eval_with_storage(graph, value, row, &ctx)?;
                let prop_val = if self.gremlin_mode {
                    match prop_val {
                        Value::Map(map) if map.contains_key("value") => {
                            Value::List(vec![Value::Map(map)])
                        }
                        other => other,
                    }
                } else {
                    prop_val
                };
                // SET n.prop = null means REMOVE the property (Cypher semantics)
                if prop_val.is_null() {
                    match val {
                        Value::Node(node_id) => {
                            self.store.remove_node_property(graph, *node_id, key)?;
                        }
                        Value::Edge(edge_id) => {
                            self.store.remove_edge_property(graph, *edge_id, key)?;
                        }
                        _ => {
                            tracing::debug!("SET property on non-node/non-edge value, ignoring");
                        }
                    }
                    return Ok(());
                }
                // Validate that the property value is a valid property type.
                // Lists-of-maps are not allowed, but standalone Maps are
                // permitted (needed for Gremlin @AllowMapPropertyValues).
                fn is_invalid_prop_value(v: &Value, gremlin_mode: bool) -> bool {
                    match v {
                        Value::List(items) => {
                            if gremlin_mode
                                && !items.is_empty()
                                && items.iter().all(
                                    |item| matches!(item, Value::Map(map) if map.contains_key("value")),
                                )
                            {
                                false
                            } else {
                                items.iter().any(|item| matches!(item, Value::Map(_)))
                            }
                        }
                        _ => false,
                    }
                }
                if is_invalid_prop_value(&prop_val, self.gremlin_mode) {
                    return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "primitive property value (not Map or List of Maps)".to_string(),
                        actual: prop_val.type_name().to_string(),
                    }));
                }
                let mut props = IndexMap::new();
                props.insert(key.clone(), prop_val);
                match val {
                    Value::Node(node_id) => {
                        self.store.set_node_properties(graph, *node_id, props)?;
                        stats.props_seen.insert((false, node_id.0, key.clone()));
                    }
                    Value::Edge(edge_id) => {
                        self.store.set_edge_properties(graph, *edge_id, props)?;
                        stats.props_seen.insert((true, edge_id.0, key.clone()));
                    }
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: format!("Node or Relationship for variable '{}'", alias.0),
                            actual: other.type_name().to_string(),
                        }));
                    }
                }
            }
            SetItem::Map { alias, map, merge } => {
                let val = row
                    .get(alias.0.as_str())
                    .ok_or_else(|| undefined_var(&alias.0))?;
                // SET on a null variable (from OPTIONAL MATCH) is silently ignored.
                if val.is_null() {
                    return Ok(());
                }
                let ctx = self.build_filter_ctx(graph, row)?;
                let map_val = self.eval_with_storage(graph, map, row, &ctx)?;
                let props = match map_val {
                    Value::Map(m) => m
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v))
                        .collect::<IndexMap<String, Value>>(),
                    Value::Null => IndexMap::new(),
                    Value::Node(src_node_id) => {
                        // SET a = b where b is a Node: copy all properties from b
                        match self.store.get_node(graph, src_node_id)? {
                            Some(node_data) => node_data.properties,
                            None => IndexMap::new(),
                        }
                    }
                    Value::Edge(src_edge_id) => {
                        // SET r = r2 where r2 is a Relationship: copy all properties from r2
                        match self.store.get_edge(graph, src_edge_id)? {
                            Some(edge_data) => edge_data.properties,
                            None => IndexMap::new(),
                        }
                    }
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "Map for SET assignment".to_string(),
                            actual: other.type_name().to_string(),
                        }));
                    }
                };
                let non_null_keys: Vec<String> = props
                    .iter()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(k, _)| k.clone())
                    .collect();
                let (is_edge, raw_id) = match val {
                    Value::Node(node_id) => {
                        let id = *node_id;
                        if *merge {
                            self.store.set_node_properties(graph, id, props)?;
                        } else {
                            self.store.replace_node_properties(graph, id, props)?;
                        }
                        (false, id.0)
                    }
                    Value::Edge(edge_id) => {
                        let id = *edge_id;
                        if *merge {
                            self.store.set_edge_properties(graph, id, props)?;
                        } else {
                            self.store.replace_edge_properties(graph, id, props)?;
                        }
                        (true, id.0)
                    }
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: format!("Node or Relationship for variable '{}'", alias.0),
                            actual: other.type_name().to_string(),
                        }));
                    }
                };
                for k in non_null_keys {
                    stats.props_seen.insert((is_edge, raw_id, k));
                }
            }
            SetItem::Labels { alias, labels } => {
                let val = row
                    .get(alias.0.as_str())
                    .ok_or_else(|| undefined_var(&alias.0))?;
                // SET labels on null (from OPTIONAL MATCH) is a no-op
                if val.is_null() {
                    return Ok(());
                }
                let node_id = extract_node_id(val, &alias.0)?;
                let label_strings: Vec<String> =
                    labels.iter().map(|l| l.as_str().to_string()).collect();
                self.store.add_node_labels(graph, node_id, &label_strings)?;
                stats.labels_seen.extend(label_strings);
            }
            SetItem::DynamicLabels { alias, value } => {
                let val = row
                    .get(alias.0.as_str())
                    .ok_or_else(|| undefined_var(&alias.0))?;
                if val.is_null() {
                    return Ok(());
                }
                let node_id = extract_node_id(val, &alias.0)?;
                let ctx = self.build_filter_ctx(graph, row)?;
                let label_val = self.eval_with_storage(graph, value, row, &ctx)?;
                let label_strings: Vec<String> = match label_val {
                    Value::Null => Vec::new(),
                    Value::Str(label) => vec![label.into_string()],
                    Value::List(items) => {
                        let mut labels = Vec::with_capacity(items.len());
                        for item in items {
                            match item {
                                Value::Str(label) => labels.push(label.into_string()),
                                other => {
                                    return Err(GqlError::Execution(
                                        ExecutionError::TypeMismatch {
                                            expected: "String or List<String> for dynamic labels"
                                                .to_string(),
                                            actual: other.type_name().to_string(),
                                        },
                                    ));
                                }
                            }
                        }
                        labels
                    }
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "String or List<String> for dynamic labels".to_string(),
                            actual: other.type_name().to_string(),
                        }));
                    }
                };
                self.store.add_node_labels(graph, node_id, &label_strings)?;
                stats.labels_seen.extend(label_strings);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr, Identifier, Label};
    use crate::executor::budget::ExecutionBudget;
    use crate::planner::{Operator, RemoveItem, SetItem};
    use crate::storage::{GraphStore, InMemoryGraphStore};

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    #[test]
    fn set_property() {
        let store = make_store();
        let nid = store
            .create_node("g", vec!["P".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Set {
            items: vec![SetItem::Property {
                alias: Identifier::new("n"),
                key: "x".to_string(),
                value: Expr::Integer(1),
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("P")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.properties_set, 1);
        assert_eq!(
            store
                .get_node("g", nid)
                .unwrap()
                .unwrap()
                .properties
                .get("x"),
            Some(&Value::Int(1))
        );
    }

    #[test]
    fn remove_property() {
        let store = make_store();
        let mut props = IndexMap::new();
        props.insert("x".to_string(), Value::Int(1));
        let nid = store
            .create_node("g", vec!["P".to_string()], props)
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Remove {
            items: vec![RemoveItem::Property {
                alias: Identifier::new("n"),
                key: "x".to_string(),
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("P")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let _ = executor.execute("g", &plan, &mut budget).unwrap();
        assert!(!store
            .get_node("g", nid)
            .unwrap()
            .unwrap()
            .properties
            .contains_key("x"));
    }

    #[test]
    fn set_null_propagates_removal_error() {
        let store = make_store();
        // Create and then delete a node so the NodeId becomes stale.
        let nid = store
            .create_node("g", vec!["P".to_string()], IndexMap::new())
            .unwrap();
        store.delete_node("g", nid, true).unwrap();

        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("n".to_string(), Value::Node(nid));
        let mut stats = MutationStats::default();

        // SET n.prop = null should attempt remove_node_property on a
        // non-existent node and propagate the StorageError.
        let item = SetItem::Property {
            alias: Identifier::new("n"),
            key: "prop".to_string(),
            value: Expr::Null,
        };
        let result = executor.apply_set_item("g", &row, &item, &mut stats);
        assert!(
            result.is_err(),
            "expected error when removing property from deleted node"
        );
    }

    #[test]
    fn set_property_on_edge() {
        use crate::ast::{Direction, RelType};
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let eid = store
            .create_edge("g", a, b, "R".to_string(), IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Set {
            items: vec![SetItem::Property {
                alias: Identifier::new("r"),
                key: "weight".to_string(),
                value: Expr::Integer(5),
            }],
            child: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("R")],
                src_alias: Identifier::new("a"),
                rel_alias: Some(Identifier::new("r")),
                dst_alias: Identifier::new("b"),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("a"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.properties_set, 1);
        let edge = store.get_edge("g", eid).unwrap().unwrap();
        assert_eq!(edge.properties.get("weight"), Some(&Value::Int(5)));
    }

    #[test]
    fn set_null_property_removes_it() {
        let store = make_store();
        let mut props = IndexMap::new();
        props.insert("x".to_string(), Value::Int(42));
        let nid = store
            .create_node("g", vec!["P".to_string()], props)
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Set {
            items: vec![SetItem::Property {
                alias: Identifier::new("n"),
                key: "x".to_string(),
                value: Expr::Null,
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("P")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let _ = executor.execute("g", &plan, &mut budget).unwrap();
        let node = store.get_node("g", nid).unwrap().unwrap();
        assert!(
            !node.properties.contains_key("x"),
            "SET prop = null should remove the property"
        );
    }

    #[test]
    fn set_labels() {
        let store = make_store();
        let nid = store
            .create_node("g", vec!["P".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Set {
            items: vec![SetItem::Labels {
                alias: Identifier::new("n"),
                labels: vec![Label::new("Extra")],
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("P")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.labels_added, 1);
        let node = store.get_node("g", nid).unwrap().unwrap();
        assert!(node.labels.contains(&"Extra".to_string()));
        assert!(node.labels.contains(&"P".to_string()));
    }

    #[test]
    fn remove_labels() {
        let store = make_store();
        let nid = store
            .create_node("g", vec!["P".to_string(), "Q".to_string()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Remove {
            items: vec![RemoveItem::Labels {
                alias: Identifier::new("n"),
                labels: vec![Label::new("Q")],
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("P")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.labels_removed, 1);
        let node = store.get_node("g", nid).unwrap().unwrap();
        assert!(node.labels.contains(&"P".to_string()));
        assert!(!node.labels.contains(&"Q".to_string()));
    }

    #[test]
    fn set_map_merge_preserves_existing() {
        let store = make_store();
        let mut props = IndexMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let nid = store
            .create_node("g", vec!["P".to_string()], props)
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Set {
            items: vec![SetItem::Map {
                alias: Identifier::new("n"),
                map: Expr::MapLiteral(vec![(smol_str::SmolStr::new("age"), Expr::Integer(30))]),
                merge: true,
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("P")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let _ = executor.execute("g", &plan, &mut budget).unwrap();
        let node = store.get_node("g", nid).unwrap().unwrap();
        assert_eq!(
            node.properties.get("name"),
            Some(&Value::Str("Alice".into()))
        );
        assert_eq!(node.properties.get("age"), Some(&Value::Int(30)));
    }

    #[test]
    fn set_map_replace_removes_old() {
        let store = make_store();
        let mut props = IndexMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        props.insert("age".to_string(), Value::Int(30));
        let nid = store
            .create_node("g", vec!["P".to_string()], props)
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Set {
            items: vec![SetItem::Map {
                alias: Identifier::new("n"),
                map: Expr::MapLiteral(vec![(
                    smol_str::SmolStr::new("title"),
                    Expr::Str("Dr".into()),
                )]),
                merge: false,
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("P")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let _ = executor.execute("g", &plan, &mut budget).unwrap();
        let node = store.get_node("g", nid).unwrap().unwrap();
        assert!(
            !node.properties.contains_key("name"),
            "old properties should be replaced"
        );
        assert!(
            !node.properties.contains_key("age"),
            "old properties should be replaced"
        );
        assert_eq!(node.properties.get("title"), Some(&Value::Str("Dr".into())));
    }

    #[test]
    fn set_on_null_variable_is_noop() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("n".to_string(), Value::Null);
        let mut stats = MutationStats::default();
        let item = SetItem::Property {
            alias: Identifier::new("n"),
            key: "x".to_string(),
            value: Expr::Integer(1),
        };
        // Should succeed silently (no-op)
        executor
            .apply_set_item("g", &row, &item, &mut stats)
            .unwrap();
        assert_eq!(stats.properties_set, 0);
    }
}
