// Benchmarks for sedan-gql-embedded's SedanGql API.
//
// Mirrors the structure of sedan-cypher-embedded/benches/engine.rs so that
// regressions and parity gaps between the two embedded entry points are
// detectable with comparable workloads. Measures the end-to-end cost of the
// single-call execute() and execute_with_context() paths.

use std::collections::HashMap;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use sedan_gql_embedded::{GraphStore, InMemoryGraphStore, ProcedureRegistry, SedanGql, Value};

// ── Setup helpers ────────────────────────────────────────────────────────────

fn seeded_store(n: usize) -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();
    let engine = SedanGql::new();
    for i in 0..n {
        engine
            .execute(
                &store,
                "g",
                &format!(
                    "INSERT (:Person {{name: 'p_{i}', age: {age}}})",
                    age = i % 100
                ),
            )
            .unwrap();
    }
    store
}

// ── End-to-end execute() ─────────────────────────────────────────────────────

fn bench_return_literal(c: &mut Criterion) {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();
    let engine = SedanGql::new();

    c.bench_function("return_literal", |b| {
        b.iter(|| engine.execute(&store, "g", "RETURN 42 AS n").unwrap());
    });
}

fn bench_create_nodes(c: &mut Criterion) {
    c.bench_function("insert_100_nodes", |b| {
        b.iter(|| {
            let store = InMemoryGraphStore::default();
            store.create_graph("g").unwrap();
            let engine = SedanGql::new();
            engine
                .execute(&store, "g", "UNWIND range(1, 100) AS i INSERT (:N {i: i})")
                .unwrap();
        });
    });
}

fn bench_match_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("match_scan");
    for size in [100usize, 1_000] {
        let store = seeded_store(size);
        let engine = SedanGql::new();
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| {
                engine
                    .execute(&store, "g", "MATCH (n:Person) RETURN count(n) AS c")
                    .unwrap()
            });
        });
    }
    group.finish();
}

fn bench_filter_and_sort(c: &mut Criterion) {
    let store = seeded_store(1_000);
    let engine = SedanGql::new();
    c.bench_function("filter_sort_1k", |b| {
        b.iter(|| {
            engine
                .execute(
                    &store,
                    "g",
                    "MATCH (n:Person) WHERE n.age > 50 RETURN n.name ORDER BY n.name LIMIT 10",
                )
                .unwrap()
        });
    });
}

fn bench_aggregation(c: &mut Criterion) {
    let store = seeded_store(1_000);
    let engine = SedanGql::new();
    c.bench_function("aggregate_group_by_1k", |b| {
        b.iter(|| {
            engine
                .execute(
                    &store,
                    "g",
                    "MATCH (n:Person) RETURN n.age, count(n) ORDER BY n.age",
                )
                .unwrap()
        });
    });
}

// ── Parameterised path (execute_with_context) ───────────────────────────────

fn bench_parameterised_query(c: &mut Criterion) {
    let store = seeded_store(500);
    let engine = SedanGql::new();
    let registry = ProcedureRegistry::new();

    let mut params: HashMap<String, Value> = HashMap::new();
    params.insert("age".into(), Value::Int(42));

    // Warm.
    engine
        .execute_with_context(
            &store,
            "g",
            "MATCH (n:Person) WHERE n.age = $age RETURN n.name",
            &registry,
            params.clone(),
        )
        .unwrap();

    c.bench_function("param_point_query_500", |b| {
        b.iter(|| {
            engine
                .execute_with_context(
                    &store,
                    "g",
                    "MATCH (n:Person) WHERE n.age = $age RETURN n.name",
                    &registry,
                    params.clone(),
                )
                .unwrap()
        });
    });
}

// ── Repeated query pattern (realistic workload) ──────────────────────────────

fn bench_repeated_query(c: &mut Criterion) {
    let store = seeded_store(500);
    let engine = SedanGql::new();
    // Warm
    engine
        .execute(
            &store,
            "g",
            "MATCH (n:Person) WHERE n.age = 42 RETURN n.name",
        )
        .unwrap();

    c.bench_function("repeated_point_query_500", |b| {
        b.iter(|| {
            engine
                .execute(
                    &store,
                    "g",
                    "MATCH (n:Person) WHERE n.age = 42 RETURN n.name",
                )
                .unwrap()
        });
    });
}

criterion_group!(
    benches,
    bench_return_literal,
    bench_create_nodes,
    bench_match_scan,
    bench_filter_and_sort,
    bench_aggregation,
    bench_parameterised_query,
    bench_repeated_query,
);
criterion_main!(benches);
