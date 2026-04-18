//! Persistence tests for PersistentGraphStore.
//!
//! Verifies WAL-based persistence: write → drop → reopen → read.
//! Uses tempdir to isolate test state.

use std::collections::HashMap;
use std::time::Duration;

use cypher_core::prelude::*;
use cypher_core::storage::{PersistentConfig, PersistentGraphStore, SyncMode};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn test_config(dir: &std::path::Path) -> PersistentConfig {
    PersistentConfig {
        data_dir: dir.to_path_buf(),
        sync_mode: SyncMode::Immediate,
        snapshot_interval: Duration::from_secs(3600), // don't auto-snapshot during tests
    }
}

fn run_on(store: &dyn GraphStore, graph: &str, query: &str) -> Vec<HashMap<String, Value>> {
    let ast = parse_query(query).expect("parse");
    let planner = QueryPlanner::new(store, graph);
    let plan = planner.plan(&ast).expect("plan");
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), usize::MAX);
    QueryExecutor::new(store)
        .execute(graph, &plan, &mut budget)
        .expect("exec")
        .rows
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[test]
fn write_close_reopen_reads_data() {
    let tmp = tempfile::tempdir().unwrap();

    // Phase 1: Write data, then drop (triggers WAL flush via Drop)
    {
        let store = PersistentGraphStore::open(test_config(tmp.path())).unwrap();
        store.create_graph("g").unwrap();
        run_on(&store, "g", "CREATE (:Person {name: 'Alice', age: 30})");
        run_on(&store, "g", "CREATE (:Person {name: 'Bob', age: 25})");
        // Drop flushes WAL
    }

    // Phase 2: Reopen and verify
    {
        let store = PersistentGraphStore::open(test_config(tmp.path())).unwrap();
        let rows = run_on(
            &store,
            "g",
            "MATCH (p:Person) RETURN p.name AS name ORDER BY name",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], Value::Str("Alice".into()));
        assert_eq!(rows[1]["name"], Value::Str("Bob".into()));
    }
}

#[test]
fn relationships_survive_restart() {
    let tmp = tempfile::tempdir().unwrap();

    {
        let store = PersistentGraphStore::open(test_config(tmp.path())).unwrap();
        store.create_graph("g").unwrap();
        run_on(
            &store,
            "g",
            "CREATE (a:Person {name: 'Alice'})-[:KNOWS]->(b:Person {name: 'Bob'})",
        );
    }

    {
        let store = PersistentGraphStore::open(test_config(tmp.path())).unwrap();
        let rows = run_on(
            &store,
            "g",
            "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name AS from, b.name AS to",
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["from"], Value::Str("Alice".into()));
        assert_eq!(rows[0]["to"], Value::Str("Bob".into()));
    }
}

#[test]
fn mutations_survive_restart() {
    let tmp = tempfile::tempdir().unwrap();

    {
        let store = PersistentGraphStore::open(test_config(tmp.path())).unwrap();
        store.create_graph("g").unwrap();
        run_on(&store, "g", "CREATE (:N {v: 1}), (:N {v: 2}), (:N {v: 3})");
        // Delete one node
        run_on(&store, "g", "MATCH (n:N {v: 2}) DELETE n");
        // Update another
        run_on(&store, "g", "MATCH (n:N {v: 1}) SET n.v = 99");
    }

    {
        let store = PersistentGraphStore::open(test_config(tmp.path())).unwrap();
        let rows = run_on(&store, "g", "MATCH (n:N) RETURN n.v AS v ORDER BY v");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["v"], Value::Int(3));
        assert_eq!(rows[1]["v"], Value::Int(99));
    }
}

#[test]
fn multiple_graphs_persist_independently() {
    let tmp = tempfile::tempdir().unwrap();

    {
        let store = PersistentGraphStore::open(test_config(tmp.path())).unwrap();
        store.create_graph("alpha").unwrap();
        store.create_graph("beta").unwrap();
        run_on(&store, "alpha", "CREATE (:N {v: 'a'})");
        run_on(&store, "beta", "CREATE (:N {v: 'b'})");
    }

    {
        let store = PersistentGraphStore::open(test_config(tmp.path())).unwrap();
        let alpha = run_on(&store, "alpha", "MATCH (n:N) RETURN n.v AS v");
        let beta = run_on(&store, "beta", "MATCH (n:N) RETURN n.v AS v");
        assert_eq!(alpha[0]["v"], Value::Str("a".into()));
        assert_eq!(beta[0]["v"], Value::Str("b".into()));
    }
}

#[test]
fn graph_stats_accurate_after_recovery() {
    let tmp = tempfile::tempdir().unwrap();

    {
        let store = PersistentGraphStore::open(test_config(tmp.path())).unwrap();
        store.create_graph("g").unwrap();
        run_on(&store, "g", "CREATE (:N {v: 1}), (:N {v: 2}), (:N {v: 3})");
        run_on(&store, "g", "CREATE (:N {v: 1})-[:E]->(:N {v: 2})");
    }

    {
        let store = PersistentGraphStore::open(test_config(tmp.path())).unwrap();
        let stats = store.graph_stats("g").unwrap();
        // 3 isolated nodes + 2 nodes from CREATE...->... = 5
        assert_eq!(stats.node_count, 5);
        assert_eq!(stats.edge_count, 1);
    }
}

#[test]
fn empty_store_creates_without_error() {
    let tmp = tempfile::tempdir().unwrap();
    let store = PersistentGraphStore::open(test_config(tmp.path())).unwrap();
    store.create_graph("g").unwrap();
    let stats = store.graph_stats("g").unwrap();
    assert_eq!(stats.node_count, 0);
    assert_eq!(stats.edge_count, 0);
}
