// Benchmarks for sedan-cypher-embedded's CypherEngine API.
//
// Measures the end-to-end cost of the single-call execute() path,
// including plan cache hit/miss behaviour.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore};

// ── Setup helpers ────────────────────────────────────────────────────────────

fn seeded_store(n: usize) -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();
    let engine = CypherEngine::new();
    for i in 0..n {
        engine
            .execute(
                &store,
                "g",
                &format!(
                    "CREATE (:Person {{name: 'p_{i}', age: {age}}})",
                    age = i % 100
                ),
            )
            .unwrap();
    }
    store
}

// ── Plan cache effectiveness ─────────────────────────────────────────────────

fn bench_cache_hit(c: &mut Criterion) {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();
    let engine = CypherEngine::new();

    // Warm the cache with a first call.
    engine.execute(&store, "g", "RETURN 42 AS n").unwrap();

    c.bench_function("cache_hit_return_literal", |b| {
        b.iter(|| engine.execute(&store, "g", "RETURN 42 AS n").unwrap());
    });
}

fn bench_cache_miss(c: &mut Criterion) {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();

    c.bench_function("cache_miss_return_literal", |b| {
        b.iter(|| {
            // Fresh engine each iteration → always a cache miss.
            let engine = CypherEngine::new();
            engine.execute(&store, "g", "RETURN 42 AS n").unwrap();
        });
    });
}

// ── CRUD through embedded API ────────────────────────────────────────────────

fn bench_create_nodes(c: &mut Criterion) {
    c.bench_function("create_100_nodes", |b| {
        b.iter(|| {
            let store = InMemoryGraphStore::default();
            store.create_graph("g").unwrap();
            let engine = CypherEngine::new();
            engine
                .execute(&store, "g", "UNWIND range(1, 100) AS i CREATE (:N {i: i})")
                .unwrap();
        });
    });
}

fn bench_match_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("match_scan");
    for size in [100usize, 1_000] {
        let store = seeded_store(size);
        let engine = CypherEngine::new();
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
    let engine = CypherEngine::new();
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
    let engine = CypherEngine::new();
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

// ── Repeated query pattern (realistic workload) ──────────────────────────────

fn bench_repeated_query(c: &mut Criterion) {
    let store = seeded_store(500);
    let engine = CypherEngine::new();
    // Warm cache
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
    bench_cache_hit,
    bench_cache_miss,
    bench_create_nodes,
    bench_match_scan,
    bench_filter_and_sort,
    bench_aggregation,
    bench_repeated_query,
);
criterion_main!(benches);
