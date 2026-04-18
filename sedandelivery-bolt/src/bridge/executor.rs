use std::collections::{HashMap, HashSet};
use std::time::Duration;

use cypher_core::error::GqlError;
use cypher_core::executor::budget::{configured_max_rows, OperatorProfileStats};
use cypher_core::executor::{ExecutionBudget, MutationStats, QueryExecutor, QueryResult};
use cypher_core::parser::parse_query;
use cypher_core::plan_cache;
use cypher_core::planner::QueryPlanner;
use cypher_core::procedure::{ProcedureDefinition, ProcedureParam, ProcedureRegistry};
use cypher_core::storage::{EdgeId, GraphStore, NodeId};
use cypher_core::types::Value;
use indexmap::IndexMap;

use super::ensure_graph_exists;
use crate::bridge::spatial::{decode_point2d, decode_point3d};
use crate::bridge::temporal::{
    decode_date, decode_datetime_offset, decode_datetime_zone_id, decode_duration,
    decode_local_datetime, decode_local_time, decode_time_with_offset,
};
use crate::bridge::types::{
    edge_to_bolt_cached, node_to_bolt_cached, path_to_bolt_cached, scalar_value_to_bolt,
    BoltGraphCache,
};
use crate::packstream::value::{BoltMap, BoltValue};
use crate::protocol::tags;

// ── Query execution ──────────────────────────────────────────────────────────

/// Execute a Cypher query against the given graph in the store.
///
/// Any graph that does not yet exist is auto-created on first use.
///
/// If `timeout_ms` is `Some`, uses that value as the query timeout in
/// milliseconds (from the Bolt `tx_timeout` field). Otherwise uses the
/// default budget.
pub fn execute_query(
    store: &dyn GraphStore,
    graph: &str,
    query: &str,
    params: HashMap<String, Value>,
    timeout_ms: Option<u64>,
) -> Result<QueryResult, GqlError> {
    // Auto-create any graph that does not yet exist.
    // sedandelivery-bolt is a standalone server with its own InMemoryGraphStore,
    // so auto-creating graphs on first use is the expected behavior —
    // there is no external catalog to consult.
    ensure_graph_exists(store, graph)?;

    let mut registry = ProcedureRegistry::default();

    // Register tx.getMetaData procedure. It returns a single row with a
    // "metadata" column containing the transaction metadata map that was
    // injected into params as __tx_metadata__.
    let tx_meta = params
        .get("__tx_metadata__")
        .cloned()
        .unwrap_or(Value::Map(IndexMap::new()));
    registry.register(ProcedureDefinition {
        name: "tx.getMetaData".to_string(),
        input_params: vec![],
        output_columns: vec![ProcedureParam {
            name: "metadata".to_string(),
            type_name: "ANY?".to_string(),
        }],
        data: vec![vec![tx_meta]],
    });

    let cache = plan_cache::global();
    let plan = if let Some((_cached_ast, cached_plan)) = cache.get(query) {
        metrics::counter!("plan_cache_hits_total").increment(1);
        cached_plan
    } else {
        metrics::counter!("plan_cache_misses_total").increment(1);
        let ast = parse_query(query)?;
        let planner = QueryPlanner::with_context(store, graph, &registry, &params);
        let plan = planner.plan(&ast)?;
        cache.insert(query, ast, plan.clone());
        plan
    };

    let executor = QueryExecutor::with_context(store, &registry, params);
    let max_rows = configured_max_rows();
    let mut budget = match timeout_ms {
        Some(ms) => ExecutionBudget::new(Duration::from_millis(ms), max_rows),
        None => ExecutionBudget::new(
            Duration::from_millis(cypher_core::executor::budget::DEFAULT_TIMEOUT_MS),
            max_rows,
        ),
    };
    executor.execute(graph, &plan, &mut budget)
}

/// Execute a Cypher query with profiling enabled, returning the result
/// and per-operator profile statistics.
///
/// The caller must also pass a `plan_annotator` closure that receives the
/// plan and profile stats while the plan is still at the same memory address
/// used during execution (important because stats are keyed by pointer address).
///
/// Returns `(QueryResult, T)` where `T` is the annotator's return value.
pub fn execute_query_with_profiling<T>(
    store: &dyn GraphStore,
    graph: &str,
    query: &str,
    params: HashMap<String, Value>,
    timeout_ms: Option<u64>,
    plan_annotator: impl FnOnce(
        &cypher_core::planner::Operator,
        Option<&HashMap<usize, OperatorProfileStats>>,
    ) -> T,
) -> Result<(QueryResult, T), GqlError> {
    ensure_graph_exists(store, graph)?;

    let mut registry = ProcedureRegistry::default();

    let tx_meta = params
        .get("__tx_metadata__")
        .cloned()
        .unwrap_or(Value::Map(IndexMap::new()));
    registry.register(ProcedureDefinition {
        name: "tx.getMetaData".to_string(),
        input_params: vec![],
        output_columns: vec![ProcedureParam {
            name: "metadata".to_string(),
            type_name: "ANY?".to_string(),
        }],
        data: vec![vec![tx_meta]],
    });

    let cache = plan_cache::global();
    let plan = if let Some((_cached_ast, cached_plan)) = cache.get(query) {
        metrics::counter!("plan_cache_hits_total").increment(1);
        cached_plan
    } else {
        metrics::counter!("plan_cache_misses_total").increment(1);
        let ast = parse_query(query)?;
        let planner = QueryPlanner::with_context(store, graph, &registry, &params);
        let plan = planner.plan(&ast)?;
        cache.insert(query, ast, plan.clone());
        plan
    };

    let executor = QueryExecutor::with_context(store, &registry, params);
    let max_rows = configured_max_rows();
    let mut budget = match timeout_ms {
        Some(ms) => ExecutionBudget::new(Duration::from_millis(ms), max_rows),
        None => ExecutionBudget::new(
            Duration::from_millis(cypher_core::executor::budget::DEFAULT_TIMEOUT_MS),
            max_rows,
        ),
    };
    budget.enable_profiling();
    let result = executor.execute(graph, &plan, &mut budget)?;
    let profile_stats = budget.take_profile_stats();

    // Annotate the plan while it's still at the same address used during execution.
    let annotation = plan_annotator(&plan, profile_stats.as_ref());

    Ok((result, annotation))
}

/// Parse and plan a Cypher query without executing it. Returns the Operator tree.
///
/// Used for EXPLAIN support: generates the query plan to return to the client.
pub fn plan_query(
    store: &dyn GraphStore,
    graph: &str,
    query: &str,
    params: HashMap<String, Value>,
) -> Result<cypher_core::planner::Operator, GqlError> {
    ensure_graph_exists(store, graph)?;

    let ast = parse_query(query)?;
    let registry = ProcedureRegistry::default();
    let planner = QueryPlanner::with_context(store, graph, &registry, &params);
    planner.plan(&ast)
}

// ── Result conversion to Bolt records ────────────────────────────────────────

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

// ── Value conversion (full: scalars + graph entities) ────────────────────────

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
        || stats.labels_added > 0
        || stats.labels_removed > 0;

    if has_writes {
        "rw"
    } else {
        "r"
    }
}

// ── Read-only query detection ────────────────────────────────────────────────

/// Check whether a Cypher query is read-only by inspecting its text for
/// mutating keywords (CREATE, SET, DELETE, MERGE, REMOVE).
///
/// This is a lightweight pre-parse check used for read-replica routing.
/// False negatives (classifying a write as a read) are not dangerous because
/// replicas will reject writes; false positives just skip the replica pool.
pub fn is_read_only_query(query: &str) -> bool {
    // Strip quoted strings (single and double) before checking for write
    // keywords, so that e.g. RETURN "CREATE" is not a false positive.
    let stripped = strip_quoted_strings(query);
    let upper = stripped.to_uppercase();
    !["CREATE", "SET", "DELETE", "MERGE", "REMOVE"]
        .iter()
        .any(|kw| upper.contains(kw))
}

/// Remove single- and double-quoted string literals from a Cypher query,
/// replacing their contents (and quotes) with spaces. This prevents write
/// keywords inside string literals from triggering false positives.
fn strip_quoted_strings(query: &str) -> String {
    let mut result = String::with_capacity(query.len());
    let mut chars = query.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\'' || ch == '"' {
            let quote = ch;
            result.push(' ');
            loop {
                match chars.next() {
                    None => break,
                    Some('\\') => {
                        // Skip the escaped character.
                        chars.next();
                    }
                    Some(c) if c == quote => break,
                    Some(_) => {}
                }
            }
        } else {
            result.push(ch);
        }
    }
    result
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
    if stats.labels_removed > 0 {
        map.push((
            "labels-removed".to_string(),
            BoltValue::Int(stats.labels_removed as i64),
        ));
    }

    map
}

// ── Bolt params to cypher-core params ───────────────────────────────────────────

/// Convert a `BoltMap` (from the Bolt RUN message's parameters field) into
/// a `HashMap<String, Value>` suitable for the cypher-core query executor.
pub fn bolt_map_to_value_map(bolt_map: &BoltMap) -> HashMap<String, Value> {
    bolt_map
        .iter()
        .map(|(k, v)| (k.clone(), bolt_value_to_gql(v)))
        .collect()
}

/// Convert a single BoltValue to a cypher-core Value.
fn bolt_value_to_gql(bv: &BoltValue) -> Value {
    match bv {
        BoltValue::Null => Value::Null,
        BoltValue::Bool(b) => Value::Bool(*b),
        BoltValue::Int(i) => Value::Int(*i),
        BoltValue::Float(f) => Value::Float(*f),
        BoltValue::String(s) => Value::Str(s.as_str().into()),
        BoltValue::Bytes(b) => Value::Bytes(b.clone()),
        BoltValue::List(items) => Value::List(items.iter().map(bolt_value_to_gql).collect()),
        BoltValue::Map(entries) => {
            let mut map = IndexMap::new();
            for (k, v) in entries {
                map.insert(Box::from(k.as_str()), bolt_value_to_gql(v));
            }
            Value::Map(map)
        }
        BoltValue::Structure { tag, fields } => bolt_structure_to_gql(*tag, fields),
    }
}

// ── Bolt Structure -> cypher-core Value decoding ────────────────────────────────

/// Decode a Bolt Structure (temporal or spatial) into a cypher-core Value.
/// Unrecognized structure tags fall back to Value::Null.
fn bolt_structure_to_gql(tag: u8, fields: &[BoltValue]) -> Value {
    match tag {
        tags::TAG_DATE => decode_date(fields),
        tags::TAG_TIME => decode_time_with_offset(fields),
        tags::TAG_LOCAL_TIME => decode_local_time(fields),
        tags::TAG_LOCAL_DATE_TIME => decode_local_datetime(fields),
        tags::TAG_DATETIME_ZONE_ID_UTC | tags::TAG_DATETIME_ZONE_ID_WALL => {
            decode_datetime_zone_id(fields)
        }
        tags::TAG_DATETIME_ZONE_OFFSET_UTC | tags::TAG_DATETIME_ZONE_OFFSET_WALL => {
            decode_datetime_offset(fields)
        }
        tags::TAG_DURATION => decode_duration(fields),
        tags::TAG_POINT_2D => decode_point2d(fields),
        tags::TAG_POINT_3D => decode_point3d(fields),
        _ => {
            tracing::error!(tag = format_args!("0x{tag:02X}"), "bolt_decode: unrecognized structure tag; this likely indicates a protocol version mismatch or corrupt data");
            Value::Null
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::types::scalar_value_to_bolt;
    use cypher_core::storage::InMemoryGraphStore;
    use cypher_core::types::value::{Crs, CypherDuration, GeoPoint};

    #[test]
    fn execute_query_auto_creates_default_graph() {
        let store = InMemoryGraphStore::new();
        assert!(!store.graph_exists("default").unwrap());

        let result =
            execute_query(&store, "default", "RETURN 1 AS x", HashMap::new(), None).unwrap();
        assert_eq!(result.columns, vec!["x".to_string()]);
        assert_eq!(result.rows.len(), 1);

        assert!(store.graph_exists("default").unwrap());
    }

    #[test]
    fn execute_query_auto_creates_non_default_graph() {
        let store = InMemoryGraphStore::new();
        // Any graph should be auto-created on first use.
        let result = execute_query(&store, "mydb", "RETURN 1 AS x", HashMap::new(), None);
        assert!(
            result.is_ok(),
            "expected auto-creation of non-default graph"
        );
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
            None,
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
    fn bolt_bytes_converts_to_bytes() {
        let bolt_map: BoltMap = vec![("data".to_string(), BoltValue::Bytes(vec![0x01, 0x02]))];
        let value_map = bolt_map_to_value_map(&bolt_map);
        assert_eq!(value_map["data"], Value::Bytes(vec![0x01, 0x02]));
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

    #[test]
    fn execute_with_params() {
        let store = InMemoryGraphStore::new();
        let mut params = HashMap::new();
        params.insert("x".to_string(), Value::Int(99));

        let result = execute_query(&store, "default", "RETURN $x AS val", params, None).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0]["val"], Value::Int(99));
    }

    // ── Structure decoding integration tests (bolt -> gql via bolt_value_to_gql) ──

    #[test]
    fn decode_date_from_bolt_structure() {
        let bv = BoltValue::Structure {
            tag: 0x44,
            fields: vec![BoltValue::Int(19737)],
        };
        let val = bolt_value_to_gql(&bv);
        let expected = jiff::civil::Date::new(2024, 1, 15).unwrap();
        assert_eq!(val, Value::Date(expected));
    }

    #[test]
    fn decode_date_epoch() {
        let bv = BoltValue::Structure {
            tag: 0x44,
            fields: vec![BoltValue::Int(0)],
        };
        let val = bolt_value_to_gql(&bv);
        let expected = jiff::civil::Date::new(1970, 1, 1).unwrap();
        assert_eq!(val, Value::Date(expected));
    }

    #[test]
    fn decode_local_time_from_bolt_structure() {
        let bv = BoltValue::Structure {
            tag: 0x74,
            fields: vec![BoltValue::Int(45_000_000_000_000)],
        };
        let val = bolt_value_to_gql(&bv);
        let expected = jiff::civil::Time::new(12, 30, 0, 0).unwrap();
        assert_eq!(val, Value::LocalTime(expected));
    }

    #[test]
    fn decode_local_time_with_nanos() {
        let nanos: i64 =
            10 * 3_600_000_000_000 + 20 * 60_000_000_000 + 30 * 1_000_000_000 + 123_456_789;
        let bv = BoltValue::Structure {
            tag: 0x74,
            fields: vec![BoltValue::Int(nanos)],
        };
        let val = bolt_value_to_gql(&bv);
        let expected = jiff::civil::Time::new(10, 20, 30, 123_456_789).unwrap();
        assert_eq!(val, Value::LocalTime(expected));
    }

    #[test]
    fn decode_local_datetime_from_bolt_structure() {
        let epoch_secs: i64 = 19737 * 86_400 + 10 * 3600 + 30 * 60 + 45;
        let bv = BoltValue::Structure {
            tag: 0x64,
            fields: vec![BoltValue::Int(epoch_secs), BoltValue::Int(123_456_789)],
        };
        let val = bolt_value_to_gql(&bv);
        let expected = jiff::civil::DateTime::new(2024, 1, 15, 10, 30, 45, 123_456_789).unwrap();
        assert_eq!(val, Value::LocalDateTime(expected));
    }

    #[test]
    fn decode_datetime_zone_id_tag_0x69() {
        let epoch_secs: i64 = 19737 * 86_400 + 10 * 3600 + 30 * 60;
        let bv = BoltValue::Structure {
            tag: 0x69,
            fields: vec![
                BoltValue::Int(epoch_secs),
                BoltValue::Int(0),
                BoltValue::String("UTC".to_string()),
            ],
        };
        let val = bolt_value_to_gql(&bv);
        if let Value::DateTime(zoned) = &val {
            assert_eq!(zoned.time_zone().iana_name(), Some("UTC"));
            assert_eq!(zoned.timestamp().as_second(), epoch_secs);
        } else {
            panic!("expected Value::DateTime, got {:?}", val);
        }
    }

    #[test]
    fn decode_datetime_zone_id_tag_0x66() {
        let epoch_secs: i64 = 19737 * 86_400 + 10 * 3600 + 30 * 60;
        let bv = BoltValue::Structure {
            tag: 0x66,
            fields: vec![
                BoltValue::Int(epoch_secs),
                BoltValue::Int(0),
                BoltValue::String("America/New_York".to_string()),
            ],
        };
        let val = bolt_value_to_gql(&bv);
        if let Value::DateTime(zoned) = &val {
            assert!(zoned.time_zone().iana_name().is_some());
        } else {
            panic!("expected Value::DateTime, got {:?}", val);
        }
    }

    #[test]
    fn decode_datetime_offset_from_bolt_structure() {
        let epoch_secs: i64 = 19737 * 86_400 + 7 * 3600 + 30 * 60;
        let bv = BoltValue::Structure {
            tag: 0x46,
            fields: vec![
                BoltValue::Int(epoch_secs),
                BoltValue::Int(0),
                BoltValue::Int(10800),
            ],
        };
        let val = bolt_value_to_gql(&bv);
        if let Value::DateTime(zoned) = &val {
            assert_eq!(zoned.offset().seconds(), 10800);
            assert_eq!(zoned.timestamp().as_second(), epoch_secs);
        } else {
            panic!("expected Value::DateTime, got {:?}", val);
        }
    }

    #[test]
    fn decode_duration_from_bolt_structure() {
        let bv = BoltValue::Structure {
            tag: 0x45,
            fields: vec![
                BoltValue::Int(14),
                BoltValue::Int(3),
                BoltValue::Int(100),
                BoltValue::Int(0),
            ],
        };
        let val = bolt_value_to_gql(&bv);
        let expected = CypherDuration {
            months: 14,
            days: 3,
            nanos: 100_000_000_000,
        };
        assert_eq!(val, Value::Duration(expected));
    }

    #[test]
    fn decode_duration_with_sub_second_nanos() {
        let bv = BoltValue::Structure {
            tag: 0x45,
            fields: vec![
                BoltValue::Int(0),
                BoltValue::Int(0),
                BoltValue::Int(5),
                BoltValue::Int(500_000_000),
            ],
        };
        let val = bolt_value_to_gql(&bv);
        let expected = CypherDuration {
            months: 0,
            days: 0,
            nanos: 5_500_000_000,
        };
        assert_eq!(val, Value::Duration(expected));
    }

    #[test]
    fn decode_point2d_wgs84_from_bolt_structure() {
        let bv = BoltValue::Structure {
            tag: 0x58,
            fields: vec![
                BoltValue::Int(4326),
                BoltValue::Float(-73.95),
                BoltValue::Float(40.78),
            ],
        };
        let val = bolt_value_to_gql(&bv);
        let expected = GeoPoint {
            x: -73.95,
            y: 40.78,
            z: None,
            crs: Crs::Wgs84,
        };
        assert_eq!(val, Value::Point(expected));
    }

    #[test]
    fn decode_point2d_cartesian_from_bolt_structure() {
        let bv = BoltValue::Structure {
            tag: 0x58,
            fields: vec![
                BoltValue::Int(7203),
                BoltValue::Float(1.5),
                BoltValue::Float(2.5),
            ],
        };
        let val = bolt_value_to_gql(&bv);
        let expected = GeoPoint {
            x: 1.5,
            y: 2.5,
            z: None,
            crs: Crs::Cartesian,
        };
        assert_eq!(val, Value::Point(expected));
    }

    #[test]
    fn decode_point3d_from_bolt_structure() {
        let bv = BoltValue::Structure {
            tag: 0x59,
            fields: vec![
                BoltValue::Int(4979),
                BoltValue::Float(1.0),
                BoltValue::Float(2.0),
                BoltValue::Float(3.0),
            ],
        };
        let val = bolt_value_to_gql(&bv);
        let expected = GeoPoint {
            x: 1.0,
            y: 2.0,
            z: Some(3.0),
            crs: Crs::Wgs843d,
        };
        assert_eq!(val, Value::Point(expected));
    }

    #[test]
    fn decode_unknown_structure_returns_null() {
        let bv = BoltValue::Structure {
            tag: 0xFF,
            fields: vec![BoltValue::Int(1)],
        };
        let val = bolt_value_to_gql(&bv);
        assert_eq!(val, Value::Null);
    }

    // ── Full round-trip tests (bolt -> gql -> bolt) ─────────────────────────

    #[test]
    fn roundtrip_date() {
        let bolt_in = BoltValue::Structure {
            tag: 0x44,
            fields: vec![BoltValue::Int(19737)],
        };
        let gql = bolt_value_to_gql(&bolt_in);
        let bolt_out = scalar_value_to_bolt(&gql);
        assert_eq!(bolt_in, bolt_out);
    }

    #[test]
    fn roundtrip_local_time() {
        let nanos: i64 = 12 * 3_600_000_000_000 + 30 * 60_000_000_000;
        let bolt_in = BoltValue::Structure {
            tag: 0x74,
            fields: vec![BoltValue::Int(nanos)],
        };
        let gql = bolt_value_to_gql(&bolt_in);
        let bolt_out = scalar_value_to_bolt(&gql);
        assert_eq!(bolt_in, bolt_out);
    }

    #[test]
    fn roundtrip_local_datetime() {
        let epoch_secs: i64 = 19737 * 86_400 + 10 * 3600 + 30 * 60 + 45;
        let bolt_in = BoltValue::Structure {
            tag: 0x64,
            fields: vec![BoltValue::Int(epoch_secs), BoltValue::Int(123_456_789)],
        };
        let gql = bolt_value_to_gql(&bolt_in);
        let bolt_out = scalar_value_to_bolt(&gql);
        assert_eq!(bolt_in, bolt_out);
    }

    #[test]
    fn roundtrip_datetime_zone_id() {
        let epoch_secs: i64 = 19737 * 86_400 + 10 * 3600 + 30 * 60;
        let bolt_in = BoltValue::Structure {
            tag: 0x69,
            fields: vec![
                BoltValue::Int(epoch_secs),
                BoltValue::Int(0),
                BoltValue::String("UTC".to_string()),
            ],
        };
        let gql = bolt_value_to_gql(&bolt_in);
        let bolt_out = scalar_value_to_bolt(&gql);
        assert_eq!(bolt_in, bolt_out);
    }

    #[test]
    fn roundtrip_datetime_offset() {
        let epoch_secs: i64 = 19737 * 86_400 + 7 * 3600 + 30 * 60;
        let bolt_in = BoltValue::Structure {
            tag: 0x49,
            fields: vec![
                BoltValue::Int(epoch_secs),
                BoltValue::Int(0),
                BoltValue::Int(10800),
            ],
        };
        let gql = bolt_value_to_gql(&bolt_in);
        let bolt_out = scalar_value_to_bolt(&gql);
        assert_eq!(bolt_in, bolt_out);
    }

    #[test]
    fn roundtrip_duration() {
        let bolt_in = BoltValue::Structure {
            tag: 0x45,
            fields: vec![
                BoltValue::Int(14),
                BoltValue::Int(3),
                BoltValue::Int(5),
                BoltValue::Int(500_000_000),
            ],
        };
        let gql = bolt_value_to_gql(&bolt_in);
        let bolt_out = scalar_value_to_bolt(&gql);
        assert_eq!(bolt_in, bolt_out);
    }

    #[test]
    fn roundtrip_point2d_wgs84() {
        let bolt_in = BoltValue::Structure {
            tag: 0x58,
            fields: vec![
                BoltValue::Int(4326),
                BoltValue::Float(-73.95),
                BoltValue::Float(40.78),
            ],
        };
        let gql = bolt_value_to_gql(&bolt_in);
        let bolt_out = scalar_value_to_bolt(&gql);
        assert_eq!(bolt_in, bolt_out);
    }

    #[test]
    fn roundtrip_point2d_cartesian() {
        let bolt_in = BoltValue::Structure {
            tag: 0x58,
            fields: vec![
                BoltValue::Int(7203),
                BoltValue::Float(1.5),
                BoltValue::Float(2.5),
            ],
        };
        let gql = bolt_value_to_gql(&bolt_in);
        let bolt_out = scalar_value_to_bolt(&gql);
        assert_eq!(bolt_in, bolt_out);
    }

    // ── End-to-end round-trip via query params ──────────────────────────────

    #[test]
    fn roundtrip_date_via_query_params() {
        let store = InMemoryGraphStore::new();
        let bolt_params: BoltMap = vec![(
            "d".to_string(),
            BoltValue::Structure {
                tag: 0x44,
                fields: vec![BoltValue::Int(19737)],
            },
        )];
        let params = bolt_map_to_value_map(&bolt_params);
        let result = execute_query(&store, "default", "RETURN $d AS d", params, None).unwrap();
        let (records, _, _, _) = result_to_records(&result, &store, "default");
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0][0],
            BoltValue::Structure {
                tag: 0x44,
                fields: vec![BoltValue::Int(19737)]
            }
        );
    }

    #[test]
    fn roundtrip_duration_via_query_params() {
        let store = InMemoryGraphStore::new();
        let bolt_params: BoltMap = vec![(
            "d".to_string(),
            BoltValue::Structure {
                tag: 0x45,
                fields: vec![
                    BoltValue::Int(14),
                    BoltValue::Int(3),
                    BoltValue::Int(100),
                    BoltValue::Int(0),
                ],
            },
        )];
        let params = bolt_map_to_value_map(&bolt_params);
        let result = execute_query(&store, "default", "RETURN $d AS d", params, None).unwrap();
        let (records, _, _, _) = result_to_records(&result, &store, "default");
        assert_eq!(
            records[0][0],
            BoltValue::Structure {
                tag: 0x45,
                fields: vec![
                    BoltValue::Int(14),
                    BoltValue::Int(3),
                    BoltValue::Int(100),
                    BoltValue::Int(0)
                ]
            }
        );
    }

    #[test]
    fn roundtrip_point_via_query_params() {
        let store = InMemoryGraphStore::new();
        let bolt_params: BoltMap = vec![(
            "p".to_string(),
            BoltValue::Structure {
                tag: 0x58,
                fields: vec![
                    BoltValue::Int(7203),
                    BoltValue::Float(1.5),
                    BoltValue::Float(2.5),
                ],
            },
        )];
        let params = bolt_map_to_value_map(&bolt_params);
        let result = execute_query(&store, "default", "RETURN $p AS p", params, None).unwrap();
        let (records, _, _, _) = result_to_records(&result, &store, "default");
        assert_eq!(
            records[0][0],
            BoltValue::Structure {
                tag: 0x58,
                fields: vec![
                    BoltValue::Int(7203),
                    BoltValue::Float(1.5),
                    BoltValue::Float(2.5)
                ]
            }
        );
    }

    #[test]
    fn streaming_error_division_by_zero_returns_partial_rows() {
        let store = InMemoryGraphStore::new();
        let result = execute_query(
            &store,
            "default",
            "UNWIND [1, 0, 2] AS x RETURN 10 / x",
            HashMap::new(),
            None,
        );
        match result {
            Ok(qr) => {
                assert_eq!(qr.rows.len(), 1, "expected 1 partial row");
                assert!(
                    qr.trailing_error.is_some(),
                    "expected trailing_error for division by zero"
                );
                let err_msg = qr.trailing_error.unwrap().to_string();
                assert!(
                    err_msg.contains("division by zero"),
                    "error should mention division by zero, got: {err_msg}"
                );
            }
            Err(e) => {
                panic!("expected Ok with partial results, got Err: {e}");
            }
        }
    }

    #[test]
    fn bolt_structure_unknown_tag_returns_null() {
        let val = bolt_structure_to_gql(0xFF, &[BoltValue::Int(1)]);
        assert_eq!(val, Value::Null);
    }

    // ── is_read_only_query tests ────────────────────────────────────────────

    #[test]
    fn is_read_only_query_match_only() {
        assert!(is_read_only_query("MATCH (n) RETURN n"));
        assert!(is_read_only_query("match (n:Person) return n.name"));
        assert!(is_read_only_query("MATCH (a)-[r]->(b) RETURN a, r, b"));
    }

    #[test]
    fn is_read_only_query_with_create() {
        assert!(!is_read_only_query("CREATE (n:Person {name: 'Alice'})"));
        assert!(!is_read_only_query("MATCH (a) CREATE (a)-[:KNOWS]->(b)"));
    }

    #[test]
    fn is_read_only_query_with_set() {
        assert!(!is_read_only_query("MATCH (n) SET n.x = 1"));
    }

    #[test]
    fn is_read_only_query_with_delete() {
        assert!(!is_read_only_query("MATCH (n) DELETE n"));
        assert!(!is_read_only_query("MATCH (n) DETACH DELETE n"));
    }

    #[test]
    fn is_read_only_query_with_merge() {
        assert!(!is_read_only_query("MERGE (n:Person {name: 'Bob'})"));
    }

    #[test]
    fn is_read_only_query_with_remove() {
        assert!(!is_read_only_query("MATCH (n) REMOVE n.prop"));
        assert!(!is_read_only_query("MATCH (n) REMOVE n:Label"));
    }

    #[test]
    fn is_read_only_query_case_insensitive() {
        assert!(!is_read_only_query("match (n) set n.x = 1"));
        assert!(!is_read_only_query("Match (n) Delete n"));
    }

    #[test]
    fn is_read_only_query_return_only() {
        assert!(is_read_only_query("RETURN 1 AS x"));
    }
}
