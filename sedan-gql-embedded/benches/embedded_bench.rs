//! High-signal embedded benchmark suite for `sedan-gql-embedded`.
//!
//! GQL (ISO/IEC 39075) parity benches mirroring `sedan-cypher-embedded` so
//! regressions and parity gaps between the two embedded entry points are
//! detectable with comparable workloads.
//!
//!   (a) parameter round-trip via `RETURN $x`
//!   (b) `MATCH (n) RETURN count(n)` over a 100-node graph
//!   (c) `MATCH (a)-[:KNOWS]->(b) RETURN a,b LIMIT 50` over 100 nodes /
//!       ~200 edges
//!   (d) plan/parse-cache hit cost (same query twice via the same engine)
//!
//! See `docs/PERFORMANCE.md` for how to run / interpret these benchmarks.

use std::collections::HashMap;
use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use sedan_gql_embedded::{GraphStore, InMemoryGraphStore, ProcedureRegistry, SedanGql, Value};

const NODES: usize = 100;
const EXTRA_EDGES: usize = 100;

fn seeded() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();
    let engine = SedanGql::new();
    for i in 0..NODES {
        engine
            .execute(
                &store,
                "g",
                &format!("INSERT (:Person {{id: {i}, name: 'p_{i}'}})"),
            )
            .unwrap();
    }
    for i in 0..(NODES - 1) {
        engine
            .execute(
                &store,
                "g",
                &format!(
                    "MATCH (a:Person {{id: {i}}}), (b:Person {{id: {j}}}) \
                     INSERT (a)-[:KNOWS]->(b)",
                    j = i + 1
                ),
            )
            .unwrap();
    }
    for i in 0..EXTRA_EDGES {
        let a = i % NODES;
        let b = (i * 7 + 3) % NODES;
        if a == b {
            continue;
        }
        engine
            .execute(
                &store,
                "g",
                &format!(
                    "MATCH (a:Person {{id: {a}}}), (b:Person {{id: {b}}}) \
                     INSERT (a)-[:KNOWS]->(b)"
                ),
            )
            .unwrap();
    }
    store
}

fn bench_param_roundtrip(c: &mut Criterion) {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();
    let engine = SedanGql::new();
    let registry = ProcedureRegistry::default();
    let mut params = HashMap::new();
    params.insert("x".to_string(), Value::Int(42));

    engine
        .execute_with_context(&store, "g", "RETURN $x AS v", &registry, params.clone())
        .unwrap();

    c.bench_function("gql/param_roundtrip", |b| {
        b.iter(|| {
            let r = engine
                .execute_with_context(
                    black_box(&store),
                    "g",
                    black_box("RETURN $x AS v"),
                    &registry,
                    params.clone(),
                )
                .unwrap();
            black_box(r);
        });
    });
}

fn bench_count_all(c: &mut Criterion) {
    let store = seeded();
    let engine = SedanGql::new();
    engine
        .execute(&store, "g", "MATCH (n) RETURN count(n) AS c")
        .unwrap();

    c.bench_function("gql/count_all_100_nodes", |b| {
        b.iter(|| {
            let r = engine
                .execute(
                    black_box(&store),
                    "g",
                    black_box("MATCH (n) RETURN count(n) AS c"),
                )
                .unwrap();
            black_box(r);
        });
    });
}

fn bench_traversal_limit(c: &mut Criterion) {
    let store = seeded();
    let engine = SedanGql::new();
    let q = "MATCH (a)-[:KNOWS]->(b) RETURN a, b LIMIT 50";
    engine.execute(&store, "g", q).unwrap();

    c.bench_function("gql/traversal_knows_limit_50", |b| {
        b.iter(|| {
            let r = engine
                .execute(black_box(&store), "g", black_box(q))
                .unwrap();
            black_box(r);
        });
    });
}

fn bench_repeat(c: &mut Criterion) {
    // SedanGql is stateless today (no engine-owned plan cache) — this bench
    // measures the steady-state "same query twice" cost so we can detect any
    // future caching wins/regressions without changing the harness.
    let store = seeded();
    let engine = SedanGql::new();
    let q = "MATCH (n:Person) WHERE n.id < 10 RETURN n.name";
    engine.execute(&store, "g", q).unwrap();

    c.bench_function("gql/repeat_same_query", |b| {
        b.iter(|| {
            let r = engine
                .execute(black_box(&store), "g", black_box(q))
                .unwrap();
            black_box(r);
        });
    });
}

criterion_group!(
    embedded,
    bench_param_roundtrip,
    bench_count_all,
    bench_traversal_limit,
    bench_repeat,
);
criterion_main!(embedded);
