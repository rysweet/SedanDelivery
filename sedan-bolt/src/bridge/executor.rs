use std::collections::HashMap;

use gql_core::error::GqlError;
use gql_core::executor::{ExecutionBudget, MutationStats, QueryExecutor, QueryResult};
use gql_core::parser::parse_query;
use gql_core::planner::QueryPlanner;
use gql_core::procedure::ProcedureRegistry;
use gql_core::storage::{GraphStore, InMemoryGraphStore};
use gql_core::types::Value;
use indexmap::IndexMap;

use crate::bridge::types::{edge_to_bolt, node_to_bolt, path_to_bolt, scalar_value_to_bolt};
use crate::packstream::value::{BoltMap, BoltValue};

// ── Query execution ──────────────────────────────────────────────────────────

/// Execute a Cypher query against the given graph in the store.
///
/// Any graph that does not yet exist is auto-created on first use.
pub fn execute_query(
    store: &InMemoryGraphStore,
    graph: &str,
    query: &str,
    params: HashMap<String, Value>,
) -> Result<QueryResult, GqlError> {
    // Auto-create any graph that does not yet exist.
    // sedan-bolt is a standalone server with its own InMemoryGraphStore,
    // so auto-creating graphs on first use is the expected behavior —
    // there is no external catalog to consult.
    let exists = store.graph_exists(graph).map_err(GqlError::from)?;
    if !exists {
        store.create_graph(graph).map_err(GqlError::from)?;
    }

    let ast = parse_query(query)?;
    let registry = ProcedureRegistry::default();
    let planner = QueryPlanner::with_context(store, graph, &registry, &params);
    let plan = planner.plan(&ast)?;

    let executor = QueryExecutor::with_context(store, &registry, params);
    let mut budget = ExecutionBudget::default_budget();
    executor.execute(graph, &plan, &mut budget)
}

// ── Result conversion to Bolt records ────────────────────────────────────────

/// Convert a `QueryResult` into Bolt-protocol records, a query type tag, and a
/// statistics map.
///
/// Returns `(records, query_type, stats_map)` where:
/// - `records` is a `Vec<Vec<BoltValue>>`, one inner vec per result row,
///   with values in column order.
/// - `query_type` is `"r"` (read-only), `"w"` (write-only), or `"rw"` (both).
/// - `stats_map` is a `BoltMap` of non-zero mutation counters.
pub fn result_to_records(
    result: &QueryResult,
    store: &InMemoryGraphStore,
    graph: &str,
) -> (Vec<Vec<BoltValue>>, String, BoltMap) {
    let records: Vec<Vec<BoltValue>> = result
        .rows
        .iter()
        .map(|row| {
            result
                .columns
                .iter()
                .map(|col| match row.get(col) {
                    Some(val) => value_to_bolt(val, store, graph),
                    None => BoltValue::Null,
                })
                .collect()
        })
        .collect();

    let query_type = determine_query_type(&result.stats).to_string();
    let stats_map = stats_to_bolt_map(&result.stats);

    (records, query_type, stats_map)
}

// ── Value conversion (full: scalars + graph entities) ────────────────────────

/// Convert a gql-core Value to a BoltValue, resolving graph entity references
/// (Node, Edge, Path) against the store.
fn value_to_bolt(val: &Value, store: &InMemoryGraphStore, graph: &str) -> BoltValue {
    match val {
        Value::Node(id) => node_to_bolt(*id, store, graph),
        Value::Edge(id) => edge_to_bolt(*id, store, graph),
        Value::Path { nodes, edges } => path_to_bolt(nodes, edges, store, graph),
        other => scalar_value_to_bolt(other),
    }
}

// ── Query type determination ─────────────────────────────────────────────────

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
        || stats.labels_added > 0;

    if has_writes {
        "rw"
    } else {
        "r"
    }
}

// ── Stats conversion ─────────────────────────────────────────────────────────

/// Convert `MutationStats` to a `BoltMap`, including only non-zero counters.
///
/// Counter names follow the Neo4j convention:
/// - `"nodes-created"`
/// - `"nodes-deleted"`
/// - `"relationships-created"`
/// - `"relationships-deleted"`
/// - `"properties-set"`
/// - `"labels-added"`
pub fn stats_to_bolt_map(stats: &MutationStats) -> BoltMap {
    let mut map: BoltMap = Vec::new();

    if stats.nodes_created > 0 {
        map.push((
            "nodes-created".to_string(),
            BoltValue::Int(stats.nodes_created as i64),
        ));
    }
    if stats.nodes_deleted > 0 {
        map.push((
            "nodes-deleted".to_string(),
            BoltValue::Int(stats.nodes_deleted as i64),
        ));
    }
    if stats.edges_created > 0 {
        map.push((
            "relationships-created".to_string(),
            BoltValue::Int(stats.edges_created as i64),
        ));
    }
    if stats.edges_deleted > 0 {
        map.push((
            "relationships-deleted".to_string(),
            BoltValue::Int(stats.edges_deleted as i64),
        ));
    }
    if stats.properties_set > 0 {
        map.push((
            "properties-set".to_string(),
            BoltValue::Int(stats.properties_set as i64),
        ));
    }
    if stats.labels_added > 0 {
        map.push((
            "labels-added".to_string(),
            BoltValue::Int(stats.labels_added as i64),
        ));
    }

    map
}

// ── Bolt params to gql-core params ───────────────────────────────────────────

/// Convert a `BoltMap` (from the Bolt RUN message's parameters field) into
/// a `HashMap<String, Value>` suitable for the gql-core query executor.
pub fn bolt_map_to_value_map(bolt_map: &BoltMap) -> HashMap<String, Value> {
    bolt_map
        .iter()
        .map(|(k, v)| (k.clone(), bolt_value_to_gql(v)))
        .collect()
}

/// Convert a single BoltValue to a gql-core Value.
fn bolt_value_to_gql(bv: &BoltValue) -> Value {
    match bv {
        BoltValue::Null => Value::Null,
        BoltValue::Bool(b) => Value::Bool(*b),
        BoltValue::Int(i) => Value::Int(*i),
        BoltValue::Float(f) => Value::Float(*f),
        BoltValue::String(s) => Value::Str(s.as_str().into()),
        BoltValue::Bytes(_) => {
            // Cypher has no native bytes type; represent as Null.
            Value::Null
        }
        BoltValue::List(items) => Value::List(items.iter().map(bolt_value_to_gql).collect()),
        BoltValue::Map(entries) => {
            let mut map = IndexMap::new();
            for (k, v) in entries {
                map.insert(Box::from(k.as_str()), bolt_value_to_gql(v));
            }
            Value::Map(map)
        }
        BoltValue::Structure { .. } => {
            // Structures in parameters are uncommon. Temporal/spatial structures
            // from Bolt clients would need dedicated decoding here. For now,
            // pass as Null -- to be extended when client-side temporal params
            // are needed.
            Value::Null
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_query_auto_creates_default_graph() {
        let store = InMemoryGraphStore::new();
        assert!(!store.graph_exists("default").unwrap());

        let result =
            execute_query(&store, "default", "RETURN 1 AS x", HashMap::new()).unwrap();
        assert_eq!(result.columns, vec!["x".to_string()]);
        assert_eq!(result.rows.len(), 1);

        assert!(store.graph_exists("default").unwrap());
    }

    #[test]
    fn execute_query_auto_creates_non_default_graph() {
        let store = InMemoryGraphStore::new();
        // Any graph should be auto-created on first use.
        let result = execute_query(&store, "mydb", "RETURN 1 AS x", HashMap::new());
        assert!(result.is_ok(), "expected auto-creation of non-default graph");
        assert!(store.graph_exists("mydb").unwrap());
    }

    #[test]
    fn execute_query_simple_return() {
        let store = InMemoryGraphStore::new();
        let result = execute_query(
            &store,
            "default",
            "RETURN 42 AS answer",
            HashMap::new(),
        )
        .unwrap();
        assert_eq!(result.columns, vec!["answer".to_string()]);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0]["answer"], Value::Int(42));
    }

    #[test]
    fn result_to_records_converts_scalars() {
        let store = InMemoryGraphStore::new();
        let result = execute_query(
            &store,
            "default",
            "RETURN 1 AS a, 'hello' AS b",
            HashMap::new(),
        )
        .unwrap();

        let (records, query_type, stats) = result_to_records(&result, &store, "default");
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
        )
        .unwrap();

        let (records, query_type, stats) = result_to_records(&result, &store, "default");
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
    fn stats_to_bolt_map_empty_for_no_mutations() {
        let stats = MutationStats::default();
        let map = stats_to_bolt_map(&stats);
        assert!(map.is_empty());
    }

    #[test]
    fn stats_to_bolt_map_includes_nonzero() {
        let mut stats = MutationStats::default();
        stats.nodes_created = 3;
        stats.edges_created = 2;
        let map = stats_to_bolt_map(&stats);
        assert_eq!(map.len(), 2);

        let has_nodes_created = map
            .iter()
            .any(|(k, v)| k == "nodes-created" && *v == BoltValue::Int(3));
        assert!(has_nodes_created);

        let has_rels_created = map
            .iter()
            .any(|(k, v)| k == "relationships-created" && *v == BoltValue::Int(2));
        assert!(has_rels_created);
    }

    #[test]
    fn bolt_map_to_value_map_basic() {
        let bolt_map: BoltMap = vec![
            ("name".to_string(), BoltValue::String("Alice".to_string())),
            ("age".to_string(), BoltValue::Int(30)),
            ("active".to_string(), BoltValue::Bool(true)),
        ];

        let value_map = bolt_map_to_value_map(&bolt_map);
        assert_eq!(value_map.len(), 3);
        assert_eq!(value_map["name"], Value::Str("Alice".into()));
        assert_eq!(value_map["age"], Value::Int(30));
        assert_eq!(value_map["active"], Value::Bool(true));
    }

    #[test]
    fn bolt_map_to_value_map_nested() {
        let inner_map: BoltMap = vec![("x".to_string(), BoltValue::Int(1))];
        let bolt_map: BoltMap = vec![
            (
                "list".to_string(),
                BoltValue::List(vec![BoltValue::Int(1), BoltValue::Int(2)]),
            ),
            ("map".to_string(), BoltValue::Map(inner_map)),
            ("null".to_string(), BoltValue::Null),
        ];

        let value_map = bolt_map_to_value_map(&bolt_map);
        assert_eq!(value_map.len(), 3);
        if let Value::List(items) = &value_map["list"] {
            assert_eq!(items.len(), 2);
        } else {
            panic!("expected List");
        }
        if let Value::Map(m) = &value_map["map"] {
            assert_eq!(m.len(), 1);
        } else {
            panic!("expected Map");
        }
        assert_eq!(value_map["null"], Value::Null);
    }

    #[test]
    fn bolt_map_to_value_map_empty() {
        let bolt_map: BoltMap = vec![];
        let value_map = bolt_map_to_value_map(&bolt_map);
        assert!(value_map.is_empty());
    }

    #[test]
    fn bolt_bytes_converts_to_null() {
        let bolt_map: BoltMap =
            vec![("data".to_string(), BoltValue::Bytes(vec![0x01, 0x02]))];
        let value_map = bolt_map_to_value_map(&bolt_map);
        assert_eq!(value_map["data"], Value::Null);
    }

    #[test]
    fn value_to_bolt_resolves_node() {
        let store = InMemoryGraphStore::new();
        store.create_graph("test").unwrap();
        let nid = store
            .create_node("test", vec!["Person".to_string()], HashMap::new())
            .unwrap();

        let bolt = value_to_bolt(&Value::Node(nid), &store, "test");
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
        let n1 = store
            .create_node("test", vec![], HashMap::new())
            .unwrap();
        let n2 = store
            .create_node("test", vec![], HashMap::new())
            .unwrap();
        let eid = store
            .create_edge("test", n1, n2, "KNOWS".to_string(), HashMap::new())
            .unwrap();

        let bolt = value_to_bolt(&Value::Edge(eid), &store, "test");
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
        assert_eq!(
            value_to_bolt(&Value::Int(42), &store, "test"),
            BoltValue::Int(42)
        );
        assert_eq!(
            value_to_bolt(&Value::Null, &store, "test"),
            BoltValue::Null
        );
    }

    #[test]
    fn execute_with_params() {
        let store = InMemoryGraphStore::new();
        let mut params = HashMap::new();
        params.insert("x".to_string(), Value::Int(99));

        let result =
            execute_query(&store, "default", "RETURN $x AS val", params).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0]["val"], Value::Int(99));
    }
}
