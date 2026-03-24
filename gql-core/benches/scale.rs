// Scale benchmarks — Phase 4 (issue #189): large-graph performance baselines.
//
// Tests graph loading throughput, label-scan performance at scale, and
// end-to-end query execution on moderately large graphs.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use gql_core::executor::{ExecutionBudget, QueryExecutor};
use gql_core::parser::parse_query;
use gql_core::planner::QueryPlanner;
use gql_core::storage::{GraphStore, InMemoryGraphStore};
use gql_core::types::Value;
use indexmap::IndexMap;
use std::time::Duration;

// ── Helpers ────────────────────────────────────────────────────────────────────

fn make_labeled_store(n: usize) -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("scale").unwrap();
    for i in 0..n {
        let props = IndexMap::from([("i".to_string(), Value::Int(i as i64))]);
        store
            .create_node("scale", vec!["Node".to_string()], props)
            .unwrap();
    }
    store
}

fn run_query(store: &InMemoryGraphStore, cypher: &str) {
    let ast = parse_query(cypher).unwrap();
    let planner = QueryPlanner::new(store, "scale");
    let plan = planner.plan(&ast).unwrap();
    let executor = QueryExecutor::new(store);
    let mut budget = ExecutionBudget::new(Duration::from_secs(120), 200_000_000);
    executor.execute("scale", &plan, &mut budget).unwrap();
}

// ── §3.1 / §4.2 Graph Loading Throughput ─────────────────────────────────────

fn bench_load_nodes(c: &mut Criterion) {
    let mut group = c.benchmark_group("load_nodes");
    for size in [1_000usize, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter(|| {
                let store = InMemoryGraphStore::default();
                store.create_graph("scale").unwrap();
                for i in 0..size {
                    let props = IndexMap::from([("i".to_string(), Value::Int(i as i64))]);
                    store
                        .create_node("scale", vec!["Node".to_string()], props)
                        .unwrap();
                }
            });
        });
    }
    group.finish();
}

// ── §4.2 Label Scan at Scale ─────────────────────────────────────────────────

fn bench_label_scan_scale(c: &mut Criterion) {
    let mut group = c.benchmark_group("label_scan");
    for size in [1_000usize, 10_000] {
        let store = make_labeled_store(size);
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| run_query(&store, "MATCH (n:Node) RETURN count(n)"));
        });
    }
    group.finish();
}

// ── §3.1 all_node_ids Performance ────────────────────────────────────────────

fn bench_all_node_ids_scale(c: &mut Criterion) {
    let mut group = c.benchmark_group("all_node_ids");
    for size in [1_000usize, 10_000] {
        let store = make_labeled_store(size);
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| store.all_node_ids("scale").unwrap());
        });
    }
    group.finish();
}

// ── §4.3 End-to-End Path Queries ─────────────────────────────────────────────

fn bench_e2e_path_1hop(c: &mut Criterion) {
    // 200-node chain; traverse all consecutive pairs (1 hop).
    let store = InMemoryGraphStore::default();
    store.create_graph("scale").unwrap();
    let mut prev = store
        .create_node("scale", vec!["Ch".to_string()], IndexMap::new())
        .unwrap();
    for _ in 0..199 {
        let next = store
            .create_node("scale", vec!["Ch".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_edge("scale", prev, next, "NEXT".to_string(), IndexMap::new())
            .unwrap();
        prev = next;
    }

    c.bench_function("e2e_path_1hop_200", |b| {
        b.iter(|| run_query(&store, "MATCH (a:Ch)-[:NEXT]->(b:Ch) RETURN count(*)"));
    });
}

fn bench_e2e_filter_at_scale(c: &mut Criterion) {
    // 5 000 nodes; filter to 5% (250 nodes).
    let store = InMemoryGraphStore::default();
    store.create_graph("scale").unwrap();
    for i in 0..5_000i64 {
        let props = IndexMap::from([
            ("i".to_string(), Value::Int(i)),
            ("hot".to_string(), Value::Bool(i % 20 == 0)),
        ]);
        store
            .create_node("scale", vec!["N".to_string()], props)
            .unwrap();
    }

    c.bench_function("e2e_filter_5k", |b| {
        b.iter(|| run_query(&store, "MATCH (n:N) WHERE n.hot = true RETURN count(n)"));
    });
}

fn bench_e2e_aggregation_at_scale(c: &mut Criterion) {
    // 3 000 nodes; group by a low-cardinality field.
    let store = InMemoryGraphStore::default();
    store.create_graph("scale").unwrap();
    for i in 0..3_000i64 {
        let props = IndexMap::from([("bucket".to_string(), Value::Int(i % 10))]);
        store
            .create_node("scale", vec!["N".to_string()], props)
            .unwrap();
    }

    c.bench_function("e2e_aggregation_3k", |b| {
        b.iter(|| {
            run_query(
                &store,
                "MATCH (n:N) RETURN n.bucket, count(n) ORDER BY n.bucket",
            )
        });
    });
}

criterion_group!(
    benches,
    bench_load_nodes,
    bench_label_scan_scale,
    bench_all_node_ids_scale,
    bench_e2e_path_1hop,
    bench_e2e_filter_at_scale,
    bench_e2e_aggregation_at_scale,
);
criterion_main!(benches);
