// Executor benchmarks — scan, filter, sort, DISTINCT, LIMIT, SKIP, UNION.
// Decomposed from benches/executor.rs (issue #1664).

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use cypher_core::executor::{ExecutionBudget, QueryExecutor};
use cypher_core::parser::parse_query;
use cypher_core::planner::QueryPlanner;
use cypher_core::storage::{GraphStore, InMemoryGraphStore};
use cypher_core::types::Value;
use indexmap::IndexMap;
use std::time::Duration;

fn make_person_store(n: usize) -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("bench").unwrap();
    for i in 0..n {
        let props = IndexMap::from([
            ("name".to_string(), Value::Str(format!("person_{i}").into())),
            ("age".to_string(), Value::Int((i % 100) as i64)),
        ]);
        store
            .create_node("bench", vec!["Person".to_string()], props)
            .unwrap();
    }
    store
}

fn run_query(store: &InMemoryGraphStore, cypher: &str) {
    let ast = parse_query(cypher).unwrap();
    let planner = QueryPlanner::new(store, "bench");
    let plan = planner.plan(&ast).unwrap();
    let executor = QueryExecutor::new(store);
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), 50_000_000);
    executor.execute("bench", &plan, &mut budget).unwrap();
}

// ── §4.2 Node / Label Scan ───────────────────────────────────────────────────

fn bench_node_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("node_scan");
    for size in [100usize, 1_000, 10_000] {
        let store = make_person_store(size);
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| run_query(&store, "MATCH (n:Person) RETURN count(n)"));
        });
    }
    group.finish();
}

// ── §4.2 Filter ──────────────────────────────────────────────────────────────

fn bench_filter_equality(c: &mut Criterion) {
    let store = make_person_store(1_000);
    c.bench_function("filter_equality_1k", |b| {
        b.iter(|| run_query(&store, "MATCH (n:Person) WHERE n.age = 42 RETURN count(n)"));
    });
}

fn bench_filter_range(c: &mut Criterion) {
    let store = make_person_store(1_000);
    c.bench_function("filter_range_1k", |b| {
        b.iter(|| run_query(&store, "MATCH (n:Person) WHERE n.age > 50 RETURN count(n)"));
    });
}

fn bench_filter_string_prefix(c: &mut Criterion) {
    let store = make_person_store(1_000);
    c.bench_function("filter_starts_with_1k", |b| {
        b.iter(|| {
            run_query(
                &store,
                "MATCH (n:Person) WHERE n.name STARTS WITH 'person_1' RETURN count(n)",
            )
        });
    });
}

// ── §4.2 Sort ────────────────────────────────────────────────────────────────

fn bench_sort(c: &mut Criterion) {
    let mut group = c.benchmark_group("sort");
    for size in [100usize, 1_000] {
        let store = make_person_store(size);
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| run_query(&store, "MATCH (n:Person) RETURN n.name ORDER BY n.name"));
        });
    }
    group.finish();
}

// ── §4.4 DISTINCT ───────────────────────────────────────────────────────────

fn bench_distinct(c: &mut Criterion) {
    let store = make_person_store(10_000);
    c.bench_function("distinct_10k_100vals", |b| {
        b.iter(|| run_query(&store, "MATCH (n:Person) RETURN DISTINCT n.age"));
    });
}

// ── §4.4 LIMIT ──────────────────────────────────────────────────────────────

fn bench_limit(c: &mut Criterion) {
    let store = make_person_store(10_000);
    c.bench_function("limit_10_of_10k", |b| {
        b.iter(|| run_query(&store, "MATCH (n:Person) RETURN n LIMIT 10"));
    });
}

// ── §4.4 SKIP ───────────────────────────────────────────────────────────────

fn bench_skip(c: &mut Criterion) {
    let store = make_person_store(10_000);
    c.bench_function("skip_9990_of_10k", |b| {
        b.iter(|| run_query(&store, "MATCH (n:Person) RETURN n SKIP 9990"));
    });
}

// ── §4.4 UNION ──────────────────────────────────────────────────────────────

fn bench_union(c: &mut Criterion) {
    let store = InMemoryGraphStore::default();
    store.create_graph("bench").unwrap();
    for i in 0..500i64 {
        store
            .create_node(
                "bench",
                vec!["A".to_string()],
                IndexMap::from([("i".to_string(), Value::Int(i))]),
            )
            .unwrap();
        store
            .create_node(
                "bench",
                vec!["B".to_string()],
                IndexMap::from([("i".to_string(), Value::Int(i))]),
            )
            .unwrap();
    }

    c.bench_function("union_500a_500b", |b| {
        b.iter(|| {
            run_query(
                &store,
                "MATCH (n:A) RETURN n.i AS val \
                 UNION \
                 MATCH (m:B) RETURN m.i AS val",
            )
        });
    });
}

criterion_group!(
    benches,
    bench_node_scan,
    bench_filter_equality,
    bench_filter_range,
    bench_filter_string_prefix,
    bench_sort,
    bench_distinct,
    bench_limit,
    bench_skip,
    bench_union,
);
criterion_main!(benches);
