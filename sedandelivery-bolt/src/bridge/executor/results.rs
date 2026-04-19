//! Convert cypher-core `QueryResult` rows into Bolt-protocol records.

use std::collections::HashSet;

use cypher_core::error::GqlError;
use cypher_core::executor::{MutationStats, QueryResult};
use cypher_core::storage::{EdgeId, GraphStore, NodeId};
use cypher_core::types::Value;

use crate::bridge::executor::stats::stats_to_bolt_map;
use crate::bridge::types::{
    edge_to_bolt_cached, node_to_bolt_cached, path_to_bolt_cached, scalar_value_to_bolt,
    BoltGraphCache,
};
use crate::packstream::value::{BoltMap, BoltValue};

/// Convert a `QueryResult` into Bolt-protocol records, a query type tag, a
/// statistics map, and an optional trailing error.
///
/// Returns `(records, query_type, stats_map, trailing_error)`.
/// - `records` is a `Vec<Vec<BoltValue>>`, one inner vec per result row,
///   with values in column order.
/// - `query_type` is `"r"` (read-only), `"w"` (write-only), or `"rw"` (both).
/// - `stats_map` is a `BoltMap` of non-zero mutation counters.
/// - `trailing_error` is an optional `GqlError` from a per-row evaluation
///   failure (e.g. division by zero) that terminated streaming after some
///   rows had already been produced.
pub fn result_to_records(
    result: &QueryResult,
    store: &dyn GraphStore,
    graph: &str,
) -> (Vec<Vec<BoltValue>>, String, BoltMap, Option<GqlError>) {
    let (node_ids, edge_ids) = collect_graph_entity_ids(result);
    let cache = BoltGraphCache::prefetch(store, graph, &node_ids, &edge_ids);
    let records: Vec<Vec<BoltValue>> = result
        .rows
        .iter()
        .map(|row| {
            result
                .columns
                .iter()
                .map(|col| match row.get(col) {
                    Some(val) => value_to_bolt(val, &cache, store, graph),
                    None => BoltValue::Null,
                })
                .collect()
        })
        .collect();

    let query_type = determine_query_type(&result.stats).to_string();
    let stats_map = stats_to_bolt_map(&result.stats);

    (
        records,
        query_type,
        stats_map,
        result.trailing_error.clone(),
    )
}

/// Convert a cypher-core Value to a BoltValue, resolving graph entity references
/// (Node, Edge, Path) against the store.
fn value_to_bolt(
    val: &Value,
    cache: &BoltGraphCache,
    store: &dyn GraphStore,
    graph: &str,
) -> BoltValue {
    match val {
        Value::Node(id) => node_to_bolt_cached(*id, cache, store, graph),
        Value::Edge(id) => edge_to_bolt_cached(*id, cache, store, graph),
        Value::Path { nodes, edges } => path_to_bolt_cached(nodes, edges, cache, store, graph),
        other => scalar_value_to_bolt(other),
    }
}

fn collect_graph_entity_ids(result: &QueryResult) -> (Vec<NodeId>, Vec<EdgeId>) {
    let mut seen_nodes = HashSet::new();
    let mut seen_edges = HashSet::new();
    let mut node_ids = Vec::new();
    let mut edge_ids = Vec::new();

    for row in &result.rows {
        for col in &result.columns {
            if let Some(value) = row.get(col) {
                collect_value_graph_ids(
                    value,
                    &mut seen_nodes,
                    &mut node_ids,
                    &mut seen_edges,
                    &mut edge_ids,
                );
            }
        }
    }

    (node_ids, edge_ids)
}

fn collect_value_graph_ids(
    value: &Value,
    seen_nodes: &mut HashSet<NodeId>,
    node_ids: &mut Vec<NodeId>,
    seen_edges: &mut HashSet<EdgeId>,
    edge_ids: &mut Vec<EdgeId>,
) {
    match value {
        Value::Node(node_id) if seen_nodes.insert(*node_id) => {
            node_ids.push(*node_id);
        }
        Value::Edge(edge_id) if seen_edges.insert(*edge_id) => {
            edge_ids.push(*edge_id);
        }
        Value::Path { nodes, edges } => {
            for node_id in nodes {
                if seen_nodes.insert(*node_id) {
                    node_ids.push(*node_id);
                }
            }
            for edge_id in edges {
                if seen_edges.insert(*edge_id) {
                    edge_ids.push(*edge_id);
                }
            }
        }
        Value::List(items) => {
            for item in items {
                collect_value_graph_ids(item, seen_nodes, node_ids, seen_edges, edge_ids);
            }
        }
        Value::Map(entries) => {
            for item in entries.values() {
                collect_value_graph_ids(item, seen_nodes, node_ids, seen_edges, edge_ids);
            }
        }
        _ => {}
    }
}

/// Determine the query type string from mutation statistics.
///
/// - `"r"` if no mutations occurred (read-only)
/// - `"rw"` if mutations occurred (reads + writes)
fn determine_query_type(stats: &MutationStats) -> &'static str {
    let has_writes = stats.nodes_created > 0
        || stats.nodes_deleted > 0
        || stats.edges_created > 0
        || stats.edges_deleted > 0
        || stats.properties_set > 0
        || stats.labels_added > 0
        || stats.labels_removed > 0;

    if has_writes {
        "rw"
    } else {
        "r"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::executor::execute_query;
    use cypher_core::storage::InMemoryGraphStore;
    use indexmap::IndexMap;
    use std::collections::HashMap;

    #[test]
    fn result_to_records_converts_scalars() {
        let store = InMemoryGraphStore::new();
        let result = execute_query(
            &store,
            "default",
            "RETURN 1 AS a, 'hello' AS b",
            HashMap::new(),
            None,
        )
        .unwrap();

        let (records, query_type, stats, _trailing) = result_to_records(&result, &store, "default");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].len(), 2);
        assert_eq!(query_type, "r");
        assert!(stats.is_empty());
    }

    #[test]
    fn result_to_records_with_writes() {
        let store = InMemoryGraphStore::new();
        let result = execute_query(
            &store,
            "default",
            "CREATE (n:Person {name: 'Alice'}) RETURN n",
            HashMap::new(),
            None,
        )
        .unwrap();

        let (records, query_type, stats, _trailing) = result_to_records(&result, &store, "default");
        assert_eq!(records.len(), 1);
        assert_eq!(query_type, "rw");
        assert!(!stats.is_empty());
    }

    #[test]
    fn determine_query_type_read_only() {
        let stats = MutationStats::default();
        assert_eq!(determine_query_type(&stats), "r");
    }

    #[test]
    fn determine_query_type_with_writes() {
        let mut stats = MutationStats::default();
        stats.nodes_created = 1;
        assert_eq!(determine_query_type(&stats), "rw");
    }

    #[test]
    fn value_to_bolt_resolves_node() {
        let store = InMemoryGraphStore::new();
        store.create_graph("test").unwrap();
        let nid = store
            .create_node("test", vec!["Person".to_string()], IndexMap::new())
            .unwrap();
        let cache = BoltGraphCache::prefetch(&store, "test", &[nid], &[]);

        let bolt = value_to_bolt(&Value::Node(nid), &cache, &store, "test");
        if let BoltValue::Structure { tag, .. } = &bolt {
            assert_eq!(*tag, 0x4E);
        } else {
            panic!("expected Structure for node");
        }
    }

    #[test]
    fn value_to_bolt_resolves_edge() {
        let store = InMemoryGraphStore::new();
        store.create_graph("test").unwrap();
        let n1 = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let n2 = store.create_node("test", vec![], IndexMap::new()).unwrap();
        let eid = store
            .create_edge("test", n1, n2, "KNOWS".to_string(), IndexMap::new())
            .unwrap();
        let cache = BoltGraphCache::prefetch(&store, "test", &[n1, n2], &[eid]);

        let bolt = value_to_bolt(&Value::Edge(eid), &cache, &store, "test");
        if let BoltValue::Structure { tag, .. } = &bolt {
            assert_eq!(*tag, 0x52);
        } else {
            panic!("expected Structure for edge");
        }
    }

    #[test]
    fn value_to_bolt_scalar_passthrough() {
        let store = InMemoryGraphStore::new();
        store.create_graph("test").unwrap();
        let cache = BoltGraphCache::prefetch(&store, "test", &[], &[]);
        assert_eq!(
            value_to_bolt(&Value::Int(42), &cache, &store, "test"),
            BoltValue::Int(42)
        );
        assert_eq!(
            value_to_bolt(&Value::Null, &cache, &store, "test"),
            BoltValue::Null
        );
    }
}
