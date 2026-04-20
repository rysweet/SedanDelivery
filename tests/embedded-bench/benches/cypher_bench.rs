//! Reliability bench — `sedan-cypher-embedded` (openCypher engine).
//!
//! All five benches run against the shared 1000-node `:Person` fixture in
//! `embedded_bench::seeded_store()`. Results are emitted under the
//! `cypher/` group so they can be diffed against `gql/` and `gremlin/`.

use std::collections::HashMap;
use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use embedded_bench::{param_schedule, seeded_store, GRAPH};
use sedan_cypher_embedded::{
    CypherEngine, GraphStore, InMemoryGraphStore, ProcedureRegistry, Value,
};

const Q_SIMPLE_MATCH: &str = "MATCH (n:Person) WHERE n.age > 21 RETURN n LIMIT 10";
const Q_TRAVERSAL_2HOP: &str = "MATCH (a)-[*2]->(c) RETURN c LIMIT 100";
const Q_AGGREGATE: &str = "MATCH (n:Person) RETURN labels(n)[0] AS label, count(n) AS c";
const Q_PARAM: &str = "MATCH (n:Person) WHERE n.age = $age RETURN n.name LIMIT 5";

fn warmed_engine(store: &InMemoryGraphStore) -> CypherEngine {
    let engine = CypherEngine::new();
    // Prime the plan cache so we measure steady-state cost.
    engine.execute(store, GRAPH, Q_SIMPLE_MATCH).unwrap();
    engine.execute(store, GRAPH, Q_TRAVERSAL_2HOP).unwrap();
    engine.execute(store, GRAPH, Q_AGGREGATE).unwrap();
    engine
}

fn bench_simple_match(c: &mut Criterion) {
    let store = seeded_store();
    let engine = warmed_engine(&store);
    c.bench_function("cypher/simple_match", |b| {
        b.iter(|| {
            let r = engine
                .execute(black_box(&store), GRAPH, black_box(Q_SIMPLE_MATCH))
                .unwrap();
            black_box(r);
        });
    });
}

fn bench_traversal_2hop(c: &mut Criterion) {
    let store = seeded_store();
    let engine = warmed_engine(&store);
    c.bench_function("cypher/traversal_2hop", |b| {
        b.iter(|| {
            let r = engine
                .execute(black_box(&store), GRAPH, black_box(Q_TRAVERSAL_2HOP))
                .unwrap();
            black_box(r);
        });
    });
}

fn bench_aggregate(c: &mut Criterion) {
    let store = seeded_store();
    let engine = warmed_engine(&store);
    c.bench_function("cypher/aggregate", |b| {
        b.iter(|| {
            let r = engine
                .execute(black_box(&store), GRAPH, black_box(Q_AGGREGATE))
                .unwrap();
            black_box(r);
        });
    });
}

fn bench_mutation(c: &mut Criterion) {
    // Insert 100 nodes per iteration into a fresh store. We rebuild the
    // store inside `iter_with_setup` so writes don't accumulate across
    // iterations (which would otherwise bias the measurement).
    c.bench_function("cypher/mutation", |b| {
        b.iter_with_setup(
            || {
                let store = InMemoryGraphStore::default();
                store.create_graph(GRAPH).unwrap();
                let engine = CypherEngine::new();
                // Prime the cache with the mutation query so we measure
                // execution-only cost (parity with the warmed read benches).
                engine
                    .execute(&store, GRAPH, "UNWIND range(1, 1) AS i CREATE (:N {i: i})")
                    .unwrap();
                (store, engine)
            },
            |(store, engine)| {
                engine
                    .execute(
                        black_box(&store),
                        GRAPH,
                        "UNWIND range(1, 100) AS i CREATE (:N {i: i})",
                    )
                    .unwrap();
            },
        );
    });
}

fn bench_parameter_binding(c: &mut Criterion) {
    let store = seeded_store();
    let engine = CypherEngine::new();
    let registry = ProcedureRegistry::new();
    let schedule = param_schedule();

    // Pre-warm: execute_with_parameters does not cache by query string,
    // but a single warm call still touches the parser/planner code paths.
    let mut warm = HashMap::new();
    warm.insert("age".to_string(), Value::Int(0));
    engine
        .execute_with_parameters(&store, GRAPH, Q_PARAM, &warm, &registry)
        .unwrap();

    c.bench_function("cypher/parameter_binding", |b| {
        b.iter(|| {
            for &age in &schedule {
                let mut params = HashMap::with_capacity(1);
                params.insert("age".to_string(), Value::Int(age));
                let r = engine
                    .execute_with_parameters(
                        black_box(&store),
                        GRAPH,
                        black_box(Q_PARAM),
                        &params,
                        &registry,
                    )
                    .unwrap();
                black_box(r);
            }
        });
    });
}

criterion_group!(
    benches,
    bench_simple_match,
    bench_traversal_2hop,
    bench_aggregate,
    bench_mutation,
    bench_parameter_binding,
);
criterion_main!(benches);
