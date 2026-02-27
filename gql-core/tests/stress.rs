//! Stress and scale tests: verify correctness and budget enforcement under load.
//!
//! Phase 3 (part 1) of the comprehensive end-to-end validation plan (issue #189).
//!
//! Covers:
//!  - §3.1 Large graph loading and statistics accuracy
//!  - §3.2 Execution budget enforcement (row limit, timeout)

use std::collections::HashMap;
use std::time::Duration;

use gql_core::error::{ExecutionError, GqlError};
use gql_core::executor::{ExecutionBudget, QueryExecutor};
use gql_core::parser::parse_query;
use gql_core::planner::QueryPlanner;
use gql_core::storage::{GraphStore, InMemoryGraphStore};
use gql_core::types::Value;

// ── Helpers ────────────────────────────────────────────────────────────────────

fn make_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("test").unwrap();
    store
}

/// Run a query with no budget limits and return rows.
fn run_query(store: &InMemoryGraphStore, query: &str) -> Vec<HashMap<String, Value>> {
    let ast = parse_query(query).expect("parse error");
    let planner = QueryPlanner::new(store, "test");
    let plan = planner.plan(&ast).expect("plan error");
    let executor = QueryExecutor::new(store);
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), usize::MAX);
    executor
        .execute("test", &plan, &mut budget)
        .expect("exec error")
        .rows
}

// ── §3.1 Large Graph Loading ──────────────────────────────────────────────────

/// Load 10 000 nodes via the storage API and verify graph stats match.
#[test]
fn test_load_10k_nodes_with_properties() {
    let store = make_store();
    let n: usize = 10_000;
    for i in 0..n {
        let props = HashMap::from([
            ("i".to_string(), Value::Int(i as i64)),
            ("label".to_string(), Value::Str(format!("node_{i}").into())),
        ]);
        store
            .create_node("test", vec!["N".to_string()], props)
            .unwrap();
    }

    let stats = store.graph_stats("test").unwrap();
    assert_eq!(
        stats.node_count, n as u64,
        "node_count must equal nodes created"
    );
    assert_eq!(stats.edge_count, 0, "no edges were created");
}

/// Graph stats must remain accurate as nodes and edges are added.
#[test]
fn test_graph_stats_accurate_at_scale() {
    let store = make_store();
    let n: usize = 500;

    // Create n nodes.
    let mut ids = vec![];
    for i in 0..n {
        let id = store
            .create_node(
                "test",
                vec!["N".to_string()],
                HashMap::from([("i".to_string(), Value::Int(i as i64))]),
            )
            .unwrap();
        ids.push(id);
    }

    // Create n edges: a chain 0→1→2→…→n-1.
    for i in 0..(n - 1) {
        store
            .create_edge(
                "test",
                ids[i],
                ids[i + 1],
                "NEXT".to_string(),
                HashMap::new(),
            )
            .unwrap();
    }

    let stats = store.graph_stats("test").unwrap();
    assert_eq!(stats.node_count, n as u64);
    assert_eq!(stats.edge_count, (n - 1) as u64);
}

/// `all_node_ids` returns the complete set of node IDs with no duplicates or gaps.
#[test]
fn test_all_node_ids_returns_complete_set() {
    let store = make_store();
    let n: usize = 300;
    let mut expected = std::collections::HashSet::new();

    for i in 0..n {
        let id = store
            .create_node(
                "test",
                vec!["N".to_string()],
                HashMap::from([("i".to_string(), Value::Int(i as i64))]),
            )
            .unwrap();
        expected.insert(id);
    }

    let returned: std::collections::HashSet<_> =
        store.all_node_ids("test").unwrap().into_iter().collect();

    assert_eq!(
        returned, expected,
        "all_node_ids must return exactly the set of created node IDs"
    );
}

/// Label scan returns all and only nodes with the given label, at moderate scale.
#[test]
fn test_label_scan_at_scale() {
    let store = make_store();

    // Create 400 "Person" nodes and 200 "Thing" nodes interleaved.
    let mut person_count = 0u64;
    for i in 0..600 {
        let label = if i % 3 == 0 { "Thing" } else { "Person" };
        if label == "Person" {
            person_count += 1;
        }
        store
            .create_node(
                "test",
                vec![label.to_string()],
                HashMap::from([("i".to_string(), Value::Int(i))]),
            )
            .unwrap();
    }

    let rows = run_query(&store, "MATCH (n:Person) RETURN count(n) AS c");
    assert_eq!(
        rows[0]["c"],
        Value::Int(person_count as i64),
        "label scan must count exactly the Person nodes"
    );
}

// ── §3.2 Budget Enforcement ───────────────────────────────────────────────────

/// Executing a query that produces more rows than the budget allows returns ResultSetTooLarge.
#[test]
fn test_row_limit_enforced() {
    let store = make_store();
    // Create 50 nodes.
    for i in 0..50i64 {
        store
            .create_node(
                "test",
                vec!["N".to_string()],
                HashMap::from([("i".to_string(), Value::Int(i))]),
            )
            .unwrap();
    }

    let ast = parse_query("MATCH (n:N) RETURN n.i").unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&ast).unwrap();
    let executor = QueryExecutor::new(&store);
    // Budget: unlimited time, but only 10 rows.
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), 10);
    let result = executor.execute("test", &plan, &mut budget);

    assert!(
        matches!(
            result,
            Err(GqlError::Execution(
                ExecutionError::ResultSetTooLarge { .. }
            ))
        ),
        "expected ResultSetTooLarge when 50 rows exceed budget of 10; got {:?}",
        result
    );
}

/// A cartesian-product query (MATCH (a), (b)) respects the row limit.
#[test]
fn test_cartesian_product_budget_enforced() {
    let store = make_store();
    // 10 nodes each: 10×10 = 100 row pairs without a budget.
    for i in 0..10i64 {
        store
            .create_node(
                "test",
                vec!["X".to_string()],
                HashMap::from([("i".to_string(), Value::Int(i))]),
            )
            .unwrap();
    }

    let ast = parse_query("MATCH (a:X), (b:X) RETURN a.i, b.i").unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&ast).unwrap();
    let executor = QueryExecutor::new(&store);
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), 20);
    let result = executor.execute("test", &plan, &mut budget);

    assert!(
        matches!(
            result,
            Err(GqlError::Execution(
                ExecutionError::ResultSetTooLarge { .. }
            ))
        ),
        "cartesian product of 100 rows must be stopped by budget of 20; got {:?}",
        result
    );
}

/// An expired deadline causes a Timeout error before any rows are yielded.
#[test]
fn test_expired_budget_returns_timeout_error() {
    let store = make_store();
    for i in 0..100i64 {
        store
            .create_node(
                "test",
                vec!["N".to_string()],
                HashMap::from([("i".to_string(), Value::Int(i))]),
            )
            .unwrap();
    }

    let ast = parse_query("MATCH (n:N) RETURN n.i").unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&ast).unwrap();
    let executor = QueryExecutor::new(&store);
    // Budget that has already expired (1 ns timeout, then sleep to ensure expiry).
    let mut budget = ExecutionBudget::new(Duration::from_nanos(1), usize::MAX);
    std::thread::sleep(Duration::from_millis(5));
    let result = executor.execute("test", &plan, &mut budget);

    assert!(
        matches!(
            result,
            Err(GqlError::Execution(ExecutionError::Timeout { .. }))
        ),
        "expired budget must produce Timeout error; got {:?}",
        result
    );
}

/// A node+edge graph at moderate scale can be fully traversed via 1-hop query.
#[test]
fn test_single_hop_traversal_at_scale() {
    let store = make_store();

    // Create 200 pairs (src -[:LINK]-> dst).
    let n = 200;
    for i in 0..n as i64 {
        let src = store
            .create_node(
                "test",
                vec!["Src".to_string()],
                HashMap::from([("i".to_string(), Value::Int(i))]),
            )
            .unwrap();
        let dst = store
            .create_node(
                "test",
                vec!["Dst".to_string()],
                HashMap::from([("i".to_string(), Value::Int(i))]),
            )
            .unwrap();
        store
            .create_edge("test", src, dst, "LINK".to_string(), HashMap::new())
            .unwrap();
    }

    let rows = run_query(
        &store,
        "MATCH (s:Src)-[:LINK]->(d:Dst) RETURN count(*) AS c",
    );
    assert_eq!(
        rows[0]["c"],
        Value::Int(n as i64),
        "traversal must find all {n} pairs"
    );
}
