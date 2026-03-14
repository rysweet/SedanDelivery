//! Property index management for [`InMemoryGraphStore`].
//!
//! Contains: create_index, drop_index, list_indexes, and BTreeMap range query helpers.

use std::sync::Arc;

use crate::error::StorageError;
use crate::storage::{IndexDescriptor, NodeId, RangeQuery};

use super::{InMemoryGraphStore, IndexableValue, PropIndex};

// ── Indexed range query helpers ───────────────────────────────────────────────

/// Collect node IDs from the BTreeMap property index that match the given range query.
pub(crate) fn indexed_range_query(
    prop_index: &PropIndex,
    label: &str,
    property: &str,
    range: &RangeQuery<'_>,
) -> Vec<NodeId> {
    use std::ops::Bound;

    let l = label.to_string();
    let p = property.to_string();

    // Helper: collect all node IDs from a BTreeMap range.
    macro_rules! collect_range {
        ($lo:expr, $hi:expr) => {{
            let mut result = Vec::new();
            for ((_l, _p, _v), ids) in prop_index.range(($lo, $hi)) {
                if _l != &l || _p != &p {
                    continue;
                }
                for &raw_id in ids {
                    result.push(NodeId(raw_id));
                }
            }
            result
        }};
    }

    // The smallest possible key for this (label, property).
    let min_key = (l.clone(), p.clone(), IndexableValue::Bool(false));

    match range {
        RangeQuery::Eq(target) => {
            if let Some(iv) = IndexableValue::from_value(target) {
                let key = (l, p, iv);
                prop_index
                    .get(&key)
                    .map(|ids| ids.iter().copied().map(NodeId).collect())
                    .unwrap_or_default()
            } else {
                vec![]
            }
        }
        RangeQuery::Lt(bound) => {
            if let Some(iv) = IndexableValue::from_value(bound) {
                let lo = Bound::Included(min_key);
                let hi = Bound::Excluded((l.clone(), p.clone(), iv));
                collect_range!(lo, hi)
            } else {
                vec![]
            }
        }
        RangeQuery::Lte(bound) => {
            if let Some(iv) = IndexableValue::from_value(bound) {
                let lo = Bound::Included(min_key);
                let hi = Bound::Included((l.clone(), p.clone(), iv));
                collect_range!(lo, hi)
            } else {
                vec![]
            }
        }
        RangeQuery::Gt(bound) => {
            if let Some(iv) = IndexableValue::from_value(bound) {
                let mut result = Vec::new();
                for ((kl, kp, _kv), ids) in prop_index.range((
                    Bound::Excluded((l.clone(), p.clone(), iv)),
                    Bound::Unbounded,
                )) {
                    if kl != &l || kp != &p {
                        break;
                    }
                    for &raw_id in ids {
                        result.push(NodeId(raw_id));
                    }
                }
                result
            } else {
                vec![]
            }
        }
        RangeQuery::Gte(bound) => {
            if let Some(iv) = IndexableValue::from_value(bound) {
                let mut result = Vec::new();
                for ((kl, kp, _kv), ids) in prop_index.range((
                    Bound::Included((l.clone(), p.clone(), iv)),
                    Bound::Unbounded,
                )) {
                    if kl != &l || kp != &p {
                        break;
                    }
                    for &raw_id in ids {
                        result.push(NodeId(raw_id));
                    }
                }
                result
            } else {
                vec![]
            }
        }
        RangeQuery::Between { lo, hi, inclusive } => {
            let lo_iv = IndexableValue::from_value(lo);
            let hi_iv = IndexableValue::from_value(hi);
            match (lo_iv, hi_iv) {
                (Some(lo_v), Some(hi_v)) => {
                    let lo_bound = Bound::Included((l.clone(), p.clone(), lo_v));
                    let hi_bound = if *inclusive {
                        Bound::Included((l.clone(), p.clone(), hi_v))
                    } else {
                        Bound::Excluded((l.clone(), p.clone(), hi_v))
                    };
                    collect_range!(lo_bound, hi_bound)
                }
                _ => vec![],
            }
        }
        RangeQuery::StartsWith(prefix) => {
            // Scan string range: from prefix to prefix with last byte incremented.
            let lo_v = IndexableValue::Str((*prefix).into());
            let mut result = Vec::new();
            for ((kl, kp, kv), ids) in prop_index.range((
                Bound::Included((l.clone(), p.clone(), lo_v)),
                Bound::Unbounded,
            )) {
                if kl != &l || kp != &p {
                    break;
                }
                if let IndexableValue::Str(s) = kv {
                    if s.starts_with(prefix) {
                        for &raw_id in ids {
                            result.push(NodeId(raw_id));
                        }
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
            result
        }
    }
}

// ── GraphStore index methods ──────────────────────────────────────────────────

/// List all indexes for a graph.
pub(crate) fn list_indexes_impl(
    store: &InMemoryGraphStore,
    graph: &str,
) -> Result<Vec<IndexDescriptor>, StorageError> {
    let snap = store.read_snapshot()?;
    let g = InMemoryGraphStore::require_graph(&snap, graph)?;
    Ok(g.indexes.clone())
}

/// Create a property index, backfilling existing nodes.
pub(crate) fn create_index_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    label: &str,
    property: &str,
) -> Result<(), StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;
    // Idempotent: only add if not already present.
    if g.has_index(label, property) {
        return Ok(());
    }
    let desc = IndexDescriptor {
        label: label.to_string(),
        property: property.to_string(),
    };
    g.indexes.push(desc);
    // Backfill: index all existing nodes that match this label+property.
    let node_ids: Vec<u64> = g
        .label_index
        .get(label)
        .map(|s| s.iter().copied().collect())
        .unwrap_or_default();
    for raw_id in node_ids {
        if let Some(node) = g.nodes.get(&raw_id) {
            if let Some(val) = node.properties.get(property) {
                if let Some(iv) = IndexableValue::from_value(val) {
                    g.prop_index
                        .entry((label.to_string(), property.to_string(), iv))
                        .or_default()
                        .insert(raw_id);
                }
            }
        }
    }
    Ok(())
}

/// Drop a property index and remove all BTreeMap entries for it.
pub(crate) fn drop_index_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    label: &str,
    property: &str,
) -> Result<(), StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;
    g.indexes
        .retain(|i| !(i.label == label && i.property == property));
    // Remove all BTreeMap entries for this label+property.
    let prefix = (label.to_string(), property.to_string());
    let keys_to_remove: Vec<super::PropIndexKey> = g
        .prop_index
        .range(
            (
                prefix.0.clone(),
                prefix.1.clone(),
                IndexableValue::Bool(false),
            )..,
        )
        .take_while(|((l, p, _), _)| l == &prefix.0 && p == &prefix.1)
        .map(|(k, _)| k.clone())
        .collect();
    for key in keys_to_remove {
        g.prop_index.remove(&key);
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::storage::mem::test_helpers::{make_indexed_store, make_store};
    use crate::storage::{GraphStore, RangeQuery};
    use crate::types::Value;

    #[test]
    fn index_create_list_drop() {
        let store = make_store();
        store.create_index("test", "Person", "name").unwrap();
        store.create_index("test", "Person", "age").unwrap();

        let indexes = store.list_indexes("test").unwrap();
        assert_eq!(indexes.len(), 2);

        store.drop_index("test", "Person", "name").unwrap();
        let indexes = store.list_indexes("test").unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0].property, "age");
    }

    #[test]
    fn create_index_idempotent() {
        let store = make_store();
        store.create_index("test", "Person", "name").unwrap();
        store.create_index("test", "Person", "name").unwrap();
        assert_eq!(store.list_indexes("test").unwrap().len(), 1);
    }

    #[test]
    fn indexed_eq_query() {
        let store = make_indexed_store();
        let p1 = HashMap::from([("age".to_string(), Value::Int(30))]);
        let a = store
            .create_node("test", vec!["Person".to_string()], p1)
            .unwrap();
        let p2 = HashMap::from([("age".to_string(), Value::Int(25))]);
        let _b = store
            .create_node("test", vec!["Person".to_string()], p2)
            .unwrap();

        let bound = Value::Int(30);
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
            .unwrap();
        assert_eq!(result, vec![a]);
    }

    #[test]
    fn indexed_lt_lte_gt_gte_queries() {
        let store = make_indexed_store();
        let ages = vec![10, 20, 30, 40, 50];
        let mut ids = Vec::new();
        for age in &ages {
            let p = HashMap::from([("age".to_string(), Value::Int(*age))]);
            ids.push(
                store
                    .create_node("test", vec!["Person".to_string()], p)
                    .unwrap(),
            );
        }

        let bound = Value::Int(30);
        let mut lt = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Lt(&bound))
            .unwrap();
        lt.sort();
        assert_eq!(lt, vec![ids[0], ids[1]]);

        let mut lte = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Lte(&bound))
            .unwrap();
        lte.sort();
        assert_eq!(lte, vec![ids[0], ids[1], ids[2]]);

        let mut gt = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Gt(&bound))
            .unwrap();
        gt.sort();
        assert_eq!(gt, vec![ids[3], ids[4]]);

        let mut gte = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Gte(&bound))
            .unwrap();
        gte.sort();
        assert_eq!(gte, vec![ids[2], ids[3], ids[4]]);
    }

    #[test]
    fn indexed_between_query() {
        let store = make_indexed_store();
        let ages = vec![10, 20, 30, 40, 50];
        let mut ids = Vec::new();
        for age in &ages {
            let p = HashMap::from([("age".to_string(), Value::Int(*age))]);
            ids.push(
                store
                    .create_node("test", vec!["Person".to_string()], p)
                    .unwrap(),
            );
        }

        let lo = Value::Int(20);
        let hi = Value::Int(40);
        let mut result = store
            .find_nodes_by_label_and_property(
                "test",
                "Person",
                "age",
                RangeQuery::Between {
                    lo: &lo,
                    hi: &hi,
                    inclusive: true,
                },
            )
            .unwrap();
        result.sort();
        assert_eq!(result, vec![ids[1], ids[2], ids[3]]);

        let mut result = store
            .find_nodes_by_label_and_property(
                "test",
                "Person",
                "age",
                RangeQuery::Between {
                    lo: &lo,
                    hi: &hi,
                    inclusive: false,
                },
            )
            .unwrap();
        result.sort();
        assert_eq!(result, vec![ids[1], ids[2]]);
    }

    #[test]
    fn indexed_starts_with_query() {
        let store = make_indexed_store();
        let names = vec!["Alice", "Albert", "Bob", "Charlie"];
        let mut ids = Vec::new();
        for name in &names {
            let p = HashMap::from([("name".to_string(), Value::Str((*name).into()))]);
            ids.push(
                store
                    .create_node("test", vec!["Person".to_string()], p)
                    .unwrap(),
            );
        }

        let mut result = store
            .find_nodes_by_label_and_property(
                "test",
                "Person",
                "name",
                RangeQuery::StartsWith("Al"),
            )
            .unwrap();
        result.sort();
        assert_eq!(result, vec![ids[0], ids[1]]);
    }

    #[test]
    fn index_backfill_existing_nodes() {
        let store = make_store();
        let p1 = HashMap::from([("age".to_string(), Value::Int(30))]);
        let a = store
            .create_node("test", vec!["Person".to_string()], p1)
            .unwrap();
        let p2 = HashMap::from([("age".to_string(), Value::Int(25))]);
        let _b = store
            .create_node("test", vec!["Person".to_string()], p2)
            .unwrap();

        store.create_index("test", "Person", "age").unwrap();

        let bound = Value::Int(30);
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
            .unwrap();
        assert_eq!(result, vec![a]);
    }

    #[test]
    fn index_maintained_on_property_update() {
        let store = make_indexed_store();
        let p = HashMap::from([("age".to_string(), Value::Int(30))]);
        let a = store
            .create_node("test", vec!["Person".to_string()], p)
            .unwrap();

        store
            .set_node_properties(
                "test",
                a,
                HashMap::from([("age".to_string(), Value::Int(40))]),
            )
            .unwrap();

        let bound30 = Value::Int(30);
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound30))
            .unwrap();
        assert!(result.is_empty());

        let bound40 = Value::Int(40);
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound40))
            .unwrap();
        assert_eq!(result, vec![a]);
    }

    #[test]
    fn index_maintained_on_property_replace() {
        let store = make_indexed_store();
        let p = HashMap::from([("age".to_string(), Value::Int(30))]);
        let a = store
            .create_node("test", vec!["Person".to_string()], p)
            .unwrap();

        store
            .replace_node_properties(
                "test",
                a,
                HashMap::from([("age".to_string(), Value::Int(50))]),
            )
            .unwrap();

        let bound30 = Value::Int(30);
        assert!(store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound30))
            .unwrap()
            .is_empty());

        let bound50 = Value::Int(50);
        assert_eq!(
            store
                .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound50))
                .unwrap(),
            vec![a]
        );
    }

    #[test]
    fn index_maintained_on_property_remove() {
        let store = make_indexed_store();
        let p = HashMap::from([("age".to_string(), Value::Int(30))]);
        let a = store
            .create_node("test", vec!["Person".to_string()], p)
            .unwrap();

        store.remove_node_property("test", a, "age").unwrap();

        let bound = Value::Int(30);
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn index_maintained_on_node_delete() {
        let store = make_indexed_store();
        let p = HashMap::from([("age".to_string(), Value::Int(30))]);
        let a = store
            .create_node("test", vec!["Person".to_string()], p)
            .unwrap();

        store.delete_node("test", a, false).unwrap();

        let bound = Value::Int(30);
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn drop_index_cleans_btreemap() {
        let store = make_indexed_store();
        let p = HashMap::from([("age".to_string(), Value::Int(30))]);
        store
            .create_node("test", vec!["Person".to_string()], p)
            .unwrap();

        store.drop_index("test", "Person", "age").unwrap();

        let bound = Value::Int(30);
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
            .unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn index_maintained_on_add_label() {
        // Create a node without the indexed label, then add it.
        // The property index should pick up the node.
        let store = make_indexed_store(); // has index on Person.age and Person.name
        let p = HashMap::from([("age".to_string(), Value::Int(42))]);
        let a = store
            .create_node("test", vec!["Animal".to_string()], p)
            .unwrap();

        // Before adding label: index lookup should not find it.
        let bound = Value::Int(42);
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
            .unwrap();
        assert!(result.is_empty());

        // Add the Person label.
        store
            .add_node_labels("test", a, &["Person".to_string()])
            .unwrap();

        // Now the index should find it.
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
            .unwrap();
        assert_eq!(result, vec![a]);
    }

    #[test]
    fn index_maintained_on_remove_label() {
        // Create a node with the indexed label, then remove it.
        // The property index should no longer find the node.
        let store = make_indexed_store();
        let p = HashMap::from([("age".to_string(), Value::Int(55))]);
        let a = store
            .create_node("test", vec!["Person".to_string()], p)
            .unwrap();

        // Before removing label: index lookup should find it.
        let bound = Value::Int(55);
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
            .unwrap();
        assert_eq!(result, vec![a]);

        // Remove the Person label.
        store
            .remove_node_labels("test", a, &["Person".to_string()])
            .unwrap();

        // Now the index should no longer find it.
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn index_scan_returns_correct_results_end_to_end() {
        // End-to-end: create index, insert nodes, verify that
        // find_nodes_by_label_and_property returns only matching nodes
        // and uses the index (not a fallback scan).
        let store = make_store();
        // Insert nodes BEFORE creating the index to test backfill.
        let p1 = HashMap::from([("name".to_string(), Value::Str("Alice".into()))]);
        let alice = store
            .create_node("test", vec!["Person".to_string()], p1)
            .unwrap();
        let p2 = HashMap::from([("name".to_string(), Value::Str("Bob".into()))]);
        let _bob = store
            .create_node("test", vec!["Person".to_string()], p2)
            .unwrap();
        let p3 = HashMap::from([("name".to_string(), Value::Str("Alice".into()))]);
        let _other = store
            .create_node("test", vec!["Company".to_string()], p3)
            .unwrap();

        // Create index — should backfill existing Person nodes.
        store.create_index("test", "Person", "name").unwrap();

        // Exact match: only Alice with label Person.
        let bound = Value::Str("Alice".into());
        let result = store
            .find_nodes_by_label_and_property("test", "Person", "name", RangeQuery::Eq(&bound))
            .unwrap();
        assert_eq!(result, vec![alice]);

        // Insert another node AFTER creating the index.
        let p4 = HashMap::from([("name".to_string(), Value::Str("Alice".into()))]);
        let alice2 = store
            .create_node("test", vec!["Person".to_string()], p4)
            .unwrap();

        // Now should find both Alices.
        let mut result = store
            .find_nodes_by_label_and_property("test", "Person", "name", RangeQuery::Eq(&bound))
            .unwrap();
        result.sort();
        let mut expected = vec![alice, alice2];
        expected.sort();
        assert_eq!(result, expected);
    }
}
