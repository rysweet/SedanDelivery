use std::collections::HashMap;
use std::time::Duration;

use gql_core::error::GqlError;
use gql_core::executor::budget::DEFAULT_MAX_ROWS;
use gql_core::executor::{ExecutionBudget, MutationStats, QueryExecutor, QueryResult};
use gql_core::parser::parse_query;
use gql_core::planner::QueryPlanner;
use gql_core::procedure::{ProcedureDefinition, ProcedureParam, ProcedureRegistry};
use gql_core::storage::GraphStore;
use gql_core::types::value::{Crs, CypherDuration, GeoPoint};
use gql_core::types::Value;
use indexmap::IndexMap;

use crate::bridge::types::{edge_to_bolt, node_to_bolt, path_to_bolt, scalar_value_to_bolt};
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
    let exists = store.graph_exists(graph).map_err(GqlError::from)?;
    if !exists {
        store.create_graph(graph).map_err(GqlError::from)?;
    }

    let ast = parse_query(query)?;
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

    let planner = QueryPlanner::with_context(store, graph, &registry, &params);
    let plan = planner.plan(&ast)?;

    let executor = QueryExecutor::with_context(store, &registry, params);
    let mut budget = match timeout_ms {
        Some(ms) => ExecutionBudget::new(Duration::from_millis(ms), DEFAULT_MAX_ROWS),
        None => ExecutionBudget::default_budget(),
    };
    executor.execute(graph, &plan, &mut budget)
}

/// Parse and plan a Cypher query without executing it. Returns the Operator tree.
///
/// Used for EXPLAIN support: generates the query plan to return to the client.
pub fn plan_query(
    store: &dyn GraphStore,
    graph: &str,
    query: &str,
    params: HashMap<String, Value>,
) -> Result<gql_core::planner::Operator, GqlError> {
    let exists = store.graph_exists(graph).map_err(GqlError::from)?;
    if !exists {
        store.create_graph(graph).map_err(GqlError::from)?;
    }

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

    (
        records,
        query_type,
        stats_map,
        result.trailing_error.clone(),
    )
}

// ── Value conversion (full: scalars + graph entities) ────────────────────────

/// Convert a gql-core Value to a BoltValue, resolving graph entity references
/// (Node, Edge, Path) against the store.
fn value_to_bolt(val: &Value, store: &dyn GraphStore, graph: &str) -> BoltValue {
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
        || stats.labels_added > 0
        || stats.labels_removed > 0;

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
    if stats.labels_removed > 0 {
        map.push((
            "labels-removed".to_string(),
            BoltValue::Int(stats.labels_removed as i64),
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

// ── Bolt Structure -> gql-core Value decoding ────────────────────────────────

/// Decode a Bolt Structure (temporal or spatial) into a gql-core Value.
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
        _ => Value::Null,
    }
}

/// Extract an i64 from a BoltValue, returning None if it is not an Int.
fn bolt_int(v: &BoltValue) -> Option<i64> {
    match v {
        BoltValue::Int(i) => Some(*i),
        _ => None,
    }
}

/// Extract an f64 from a BoltValue (accepts both Int and Float).
fn bolt_float(v: &BoltValue) -> Option<f64> {
    match v {
        BoltValue::Float(f) => Some(*f),
        BoltValue::Int(i) => Some(*i as f64),
        _ => None,
    }
}

/// Date (tag 0x44): 1 field -- days since 1970-01-01.
fn decode_date(fields: &[BoltValue]) -> Value {
    let days = match fields.first().and_then(bolt_int) {
        Some(d) => d,
        None => return Value::Null,
    };
    let epoch = jiff::civil::Date::new(1970, 1, 1).expect("valid epoch");
    match epoch.checked_add(jiff::Span::new().days(days)) {
        Ok(d) => Value::Date(d),
        Err(_) => Value::Null,
    }
}

/// LocalTime (tag 0x74): 1 field -- nanoseconds since midnight.
fn decode_local_time(fields: &[BoltValue]) -> Value {
    let nanos = match fields.first().and_then(bolt_int) {
        Some(n) => n,
        None => return Value::Null,
    };
    let h = (nanos / 3_600_000_000_000) as i8;
    let rem = nanos % 3_600_000_000_000;
    let m = (rem / 60_000_000_000) as i8;
    let rem = rem % 60_000_000_000;
    let s = (rem / 1_000_000_000) as i8;
    let ns = (rem % 1_000_000_000) as i32;
    match jiff::civil::Time::new(h, m, s, ns) {
        Ok(t) => Value::LocalTime(t),
        Err(_) => Value::Null,
    }
}

/// Time with UTC offset (tag 0x54): 2 fields -- nanoseconds since midnight, offset_seconds.
/// Returns a Value::Time (time + offset) representing a zoned time with no date component.
fn decode_time_with_offset(fields: &[BoltValue]) -> Value {
    let nanos = match fields.first().and_then(bolt_int) {
        Some(n) => n,
        None => return Value::Null,
    };
    let offset_secs = match fields.get(1).and_then(bolt_int) {
        Some(o) => o,
        None => return Value::Null,
    };

    let h = (nanos / 3_600_000_000_000) as i8;
    let rem = nanos % 3_600_000_000_000;
    let m = (rem / 60_000_000_000) as i8;
    let rem = rem % 60_000_000_000;
    let s = (rem / 1_000_000_000) as i8;
    let ns = (rem % 1_000_000_000) as i32;

    match jiff::civil::Time::new(h, m, s, ns) {
        Ok(t) => Value::Time(t, offset_secs as i32),
        Err(_) => Value::Null,
    }
}

/// LocalDateTime (tag 0x64): 2 fields -- epoch_seconds, nanosecond_adjustment.
fn decode_local_datetime(fields: &[BoltValue]) -> Value {
    let epoch_secs = match fields.first().and_then(bolt_int) {
        Some(s) => s,
        None => return Value::Null,
    };
    let nanos_adj = fields.get(1).and_then(bolt_int).unwrap_or(0);

    // Decompose epoch_secs into days + time-of-day seconds.
    // Handle negative epoch_secs correctly (dates before 1970).
    let (days, day_secs) = if epoch_secs >= 0 {
        (epoch_secs / 86_400, epoch_secs % 86_400)
    } else {
        // Floor division for negative values.
        let d = (epoch_secs - 86_399) / 86_400;
        let s = epoch_secs - d * 86_400;
        (d, s)
    };

    let epoch = jiff::civil::Date::new(1970, 1, 1).expect("valid epoch");
    let date = match epoch.checked_add(jiff::Span::new().days(days)) {
        Ok(d) => d,
        Err(_) => return Value::Null,
    };

    let h = (day_secs / 3_600) as i8;
    let rem = day_secs % 3_600;
    let m = (rem / 60) as i8;
    let s = (rem % 60) as i8;

    let time = match jiff::civil::Time::new(h, m, s, nanos_adj as i32) {
        Ok(t) => t,
        Err(_) => return Value::Null,
    };

    Value::LocalDateTime(jiff::civil::DateTime::from_parts(date, time))
}

/// DateTime with zone ID (tag 0x49/0x69): 3 fields -- epoch_secs, nanos, tz_id string.
fn decode_datetime_zone_id(fields: &[BoltValue]) -> Value {
    let epoch_secs = match fields.first().and_then(bolt_int) {
        Some(s) => s,
        None => return Value::Null,
    };
    let nanos_adj = fields.get(1).and_then(bolt_int).unwrap_or(0);
    let tz_id = match fields.get(2) {
        Some(BoltValue::String(s)) => s.as_str(),
        _ => return Value::Null,
    };

    let tz = match jiff::tz::TimeZone::get(tz_id) {
        Ok(tz) => tz,
        Err(_) => return Value::Null,
    };

    let ts = match jiff::Timestamp::new(epoch_secs, nanos_adj as i32) {
        Ok(ts) => ts,
        Err(_) => return Value::Null,
    };

    let zoned = ts.to_zoned(tz);
    Value::DateTime(Box::new(zoned))
}

/// DateTime with offset (tag 0x46/0x66): 3 fields -- epoch_secs, nanos, offset_seconds.
fn decode_datetime_offset(fields: &[BoltValue]) -> Value {
    let epoch_secs = match fields.first().and_then(bolt_int) {
        Some(s) => s,
        None => return Value::Null,
    };
    let nanos_adj = fields.get(1).and_then(bolt_int).unwrap_or(0);
    let offset_secs = match fields.get(2).and_then(bolt_int) {
        Some(o) => o,
        None => return Value::Null,
    };

    let offset = match jiff::tz::Offset::from_seconds(offset_secs as i32) {
        Ok(o) => o,
        Err(_) => return Value::Null,
    };
    let tz = jiff::tz::TimeZone::fixed(offset);

    let ts = match jiff::Timestamp::new(epoch_secs, nanos_adj as i32) {
        Ok(ts) => ts,
        Err(_) => return Value::Null,
    };

    let zoned = ts.to_zoned(tz);
    Value::DateTime(Box::new(zoned))
}

/// Duration (tag 0x45): 4 fields -- months, days, seconds, nanoseconds.
fn decode_duration(fields: &[BoltValue]) -> Value {
    let months = fields.first().and_then(bolt_int).unwrap_or(0);
    let days = fields.get(1).and_then(bolt_int).unwrap_or(0);
    let seconds = fields.get(2).and_then(bolt_int).unwrap_or(0);
    let nanos = fields.get(3).and_then(bolt_int).unwrap_or(0);

    Value::Duration(CypherDuration {
        months,
        days,
        nanos: (seconds as i128) * 1_000_000_000 + (nanos as i128),
    })
}

/// Map an SRID integer to a Crs variant.
fn srid_to_crs(srid: i64) -> Crs {
    match srid {
        4326 => Crs::Wgs84,
        4979 => Crs::Wgs843d,
        7203 => Crs::Cartesian,
        9157 => Crs::Cartesian3d,
        // Default to Cartesian for unknown SRIDs.
        _ => Crs::Cartesian,
    }
}

/// Point2D (tag 0x58): 3 fields -- srid, x, y.
fn decode_point2d(fields: &[BoltValue]) -> Value {
    let srid = match fields.first().and_then(bolt_int) {
        Some(s) => s,
        None => return Value::Null,
    };
    let x = match fields.get(1).and_then(bolt_float) {
        Some(v) => v,
        None => return Value::Null,
    };
    let y = match fields.get(2).and_then(bolt_float) {
        Some(v) => v,
        None => return Value::Null,
    };

    Value::Point(GeoPoint {
        x,
        y,
        z: None,
        crs: srid_to_crs(srid),
    })
}

/// Point3D (tag 0x59): 4 fields -- srid, x, y, z.
fn decode_point3d(fields: &[BoltValue]) -> Value {
    let srid = match fields.first().and_then(bolt_int) {
        Some(s) => s,
        None => return Value::Null,
    };
    let x = match fields.get(1).and_then(bolt_float) {
        Some(v) => v,
        None => return Value::Null,
    };
    let y = match fields.get(2).and_then(bolt_float) {
        Some(v) => v,
        None => return Value::Null,
    };
    let z = fields.get(3).and_then(bolt_float);

    Value::Point(GeoPoint {
        x,
        y,
        z,
        crs: srid_to_crs(srid),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gql_core::storage::InMemoryGraphStore;

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
        let n1 = store.create_node("test", vec![], HashMap::new()).unwrap();
        let n2 = store.create_node("test", vec![], HashMap::new()).unwrap();
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
        assert_eq!(value_to_bolt(&Value::Null, &store, "test"), BoltValue::Null);
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

    // ── Structure decoding tests (bolt -> gql) ──────────────────────────────

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
        // 0x69 (lowercase 'i') = UTC-mode DateTimeZoneId: (epoch_secs, nanos, tz_string)
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
        // 0x66 (lowercase 'f') = wall-clock DateTimeZoneId: (epoch_secs, nanos, tz_string)
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
        // UTC-mode zone name uses tag 0x69 (lowercase 'i')
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
        // UTC-mode offset uses tag 0x49 (uppercase 'I')
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
                fields: vec![BoltValue::Int(19737)],
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
                    BoltValue::Int(0),
                ],
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
                    BoltValue::Float(2.5),
                ],
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
                // We should get 1 partial row (10/1=10), then a trailing error.
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
}
