//! High-signal embedded benchmark suite for `sedan-cypher-embedded`.
//!
//! Measures the four headline scenarios users care about when embedding the
//! Cypher engine in-process:
//!   (a) parameter round-trip via `RETURN $x` (parser/planner overhead floor)
//!   (b) `MATCH (n) RETURN count(n)` over a 100-node graph (full scan)
//!   (c) `MATCH (a)-[:KNOWS]->(b) RETURN a,b LIMIT 50` over 100 nodes /
//!       200 edges (label+rel-type filtered traversal with limit)
//!   (d) plan-cache hit cost vs. miss (same query twice)
//!
//! See `docs/PERFORMANCE.md` for how to run / interpret these benchmarks.

use std::collections::HashMap;
use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use sedan_cypher_embedded::{
    CypherEngine, GraphStore, InMemoryGraphStore, ProcedureRegistry, Value,
};

const NODES: usize = 100;
const EXTRA_EDGES: usize = 100; // total ≈200 KNOWS edges (chain + extras)

fn seeded() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();
    let engine = CypherEngine::new();
    for i in 0..NODES {
        engine
            .execute(
                &store,
                "g",
                &format!("CREATE (:Person {{id: {i}, name: 'p_{i}'}})"),
            )
            .unwrap();
    }
    // Chain: p_i -[:KNOWS]-> p_{i+1}  (NODES-1 edges)
    for i in 0..(NODES - 1) {
        engine
            .execute(
                &store,
                "g",
                &format!(
                    "MATCH (a:Person {{id: {i}}}), (b:Person {{id: {j}}}) \
                     CREATE (a)-[:KNOWS]->(b)",
                    j = i + 1
                ),
            )
            .unwrap();
    }
    // Extra cross-edges so traversal benchmark has fan-out.
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
                     CREATE (a)-[:KNOWS]->(b)"
                ),
            )
            .unwrap();
    }
    store
}

fn bench_param_roundtrip(c: &mut Criterion) {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();
    let engine = CypherEngine::new();
    let registry = ProcedureRegistry::default();
    let mut params = HashMap::new();
    params.insert("x".to_string(), Value::Int(42));

    // Warm cache so we measure steady-state.
    engine
        .execute_with_parameters(&store, "g", "RETURN $x AS v", &params, &registry)
        .unwrap();

    c.bench_function("cypher/param_roundtrip", |b| {
        b.iter(|| {
            let r = engine
                .execute_with_parameters(
                    black_box(&store),
                    "g",
                    black_box("RETURN $x AS v"),
                    black_box(&params),
                    &registry,
                )
                .unwrap();
            black_box(r);
        });
    });
}

fn bench_count_all(c: &mut Criterion) {
    let store = seeded();
    let engine = CypherEngine::new();
    engine
        .execute(&store, "g", "MATCH (n) RETURN count(n) AS c")
        .unwrap();

    c.bench_function("cypher/count_all_100_nodes", |b| {
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
    let engine = CypherEngine::new();
    let q = "MATCH (a)-[:KNOWS]->(b) RETURN a, b LIMIT 50";
    engine.execute(&store, "g", q).unwrap();

    c.bench_function("cypher/traversal_knows_limit_50", |b| {
        b.iter(|| {
            let r = engine
                .execute(black_box(&store), "g", black_box(q))
                .unwrap();
            black_box(r);
        });
    });
}

fn bench_plan_cache(c: &mut Criterion) {
    let mut group = c.benchmark_group("cypher/plan_cache");
    let store = seeded();
    let q = "MATCH (n:Person) WHERE n.id < 10 RETURN n.name";

    // Cold: fresh engine each iteration → always parses & plans.
    group.bench_function("miss", |b| {
        b.iter(|| {
            let engine = CypherEngine::new();
            let r = engine
                .execute(black_box(&store), "g", black_box(q))
                .unwrap();
            black_box(r);
        });
    });

    // Hot: reuse engine → plan cache hit on every iteration.
    let engine = CypherEngine::new();
    engine.execute(&store, "g", q).unwrap();
    group.bench_function("hit", |b| {
        b.iter(|| {
            let r = engine
                .execute(black_box(&store), "g", black_box(q))
                .unwrap();
            black_box(r);
        });
    });

    group.finish();
}

criterion_group!(
    embedded,
    bench_param_roundtrip,
    bench_count_all,
    bench_traversal_limit,
    bench_plan_cache,
);
criterion_main!(embedded);
