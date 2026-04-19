// Executor benchmarks — CREATE throughput and MERGE.
// Decomposed from benches/executor.rs (issue #1664).

use criterion::{criterion_group, criterion_main, Criterion};
use cypher_core::executor::{ExecutionBudget, QueryExecutor};
use cypher_core::parser::parse_query;
use cypher_core::planner::QueryPlanner;
use cypher_core::storage::{GraphStore, InMemoryGraphStore};
use cypher_core::types::Value;
use indexmap::IndexMap;
use std::time::Duration;

fn run_query(store: &InMemoryGraphStore, cypher: &str) {
    let ast = parse_query(cypher).unwrap();
    let planner = QueryPlanner::new(store, "bench");
    let plan = planner.plan(&ast).unwrap();
    let executor = QueryExecutor::new(store);
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), 50_000_000);
    executor.execute("bench", &plan, &mut budget).unwrap();
}

// ── §4.2 CREATE Throughput ───────────────────────────────────────────────────

fn bench_create_node_throughput(c: &mut Criterion) {
    c.bench_function("create_100_nodes", |b| {
        b.iter(|| {
            let store = InMemoryGraphStore::default();
            store.create_graph("bench").unwrap();
            run_query(&store, "UNWIND range(1, 100) AS i CREATE (:Node {i: i})");
        });
    });
}

// ── §4.4 MERGE ──────────────────────────────────────────────────────────────

fn bench_merge(c: &mut Criterion) {
    // MERGE on existing data — measures match-existing-node path.
    // Each iteration creates a fresh store with pre-existing data, then MERGEs.
    c.bench_function("merge_existing_100", |b| {
        b.iter(|| {
            let store = InMemoryGraphStore::default();
            store.create_graph("bench").unwrap();
            for i in 0..100i64 {
                let props =
                    IndexMap::from([("name".to_string(), Value::Str(format!("node_{i}").into()))]);
                store
                    .create_node("bench", vec!["Label".to_string()], props)
                    .unwrap();
            }
            // MERGE should find existing node, not create a new one.
            run_query(&store, "MERGE (n:Label {name: 'node_50'}) RETURN n.name");
        });
    });
}

criterion_group!(benches, bench_create_node_throughput, bench_merge);
criterion_main!(benches);
