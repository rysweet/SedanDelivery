//! Reliability bench — `sedan-gql-embedded` (GQL/Cypher engine).
//!
//! Mirrors `cypher_bench.rs` so per-bench results are directly comparable
//! across the three engines. All benches use the shared 1000-node fixture.

use std::collections::HashMap;
use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use embedded_bench::{param_schedule, seeded_store, GRAPH};
use sedan_gql_embedded::{GraphStore, InMemoryGraphStore, ProcedureRegistry, SedanGql, Value};

const Q_SIMPLE_MATCH: &str = "MATCH (n:Person) WHERE n.age > 21 RETURN n LIMIT 10";
const Q_TRAVERSAL_2HOP: &str = "MATCH (a)-[*2]->(c) RETURN c LIMIT 100";
const Q_AGGREGATE: &str = "MATCH (n:Person) RETURN labels(n)[0] AS label, count(n) AS c";
const Q_PARAM: &str = "MATCH (n:Person) WHERE n.age = $age RETURN n.name LIMIT 5";

fn bench_simple_match(c: &mut Criterion) {
    let store = seeded_store();
    let engine = SedanGql::new();
    engine.execute(&store, GRAPH, Q_SIMPLE_MATCH).unwrap();
    c.bench_function("gql/simple_match", |b| {
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
    let engine = SedanGql::new();
    engine.execute(&store, GRAPH, Q_TRAVERSAL_2HOP).unwrap();
    c.bench_function("gql/traversal_2hop", |b| {
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
    let engine = SedanGql::new();
    engine.execute(&store, GRAPH, Q_AGGREGATE).unwrap();
    c.bench_function("gql/aggregate", |b| {
        b.iter(|| {
            let r = engine
                .execute(black_box(&store), GRAPH, black_box(Q_AGGREGATE))
                .unwrap();
            black_box(r);
        });
    });
}

fn bench_mutation(c: &mut Criterion) {
    c.bench_function("gql/mutation", |b| {
        b.iter_with_setup(
            || {
                let store = InMemoryGraphStore::default();
                store.create_graph(GRAPH).unwrap();
                let engine = SedanGql::new();
                engine
                    .execute(&store, GRAPH, "UNWIND range(1, 1) AS i INSERT (:N {i: i})")
                    .unwrap();
                (store, engine)
            },
            |(store, engine)| {
                engine
                    .execute(
                        black_box(&store),
                        GRAPH,
                        "UNWIND range(1, 100) AS i INSERT (:N {i: i})",
                    )
                    .unwrap();
            },
        );
    });
}

fn bench_parameter_binding(c: &mut Criterion) {
    let store = seeded_store();
    let engine = SedanGql::new();
    let registry = ProcedureRegistry::new();
    let schedule = param_schedule();

    let mut warm = HashMap::new();
    warm.insert("age".to_string(), Value::Int(0));
    engine
        .execute_with_context(&store, GRAPH, Q_PARAM, &registry, warm)
        .unwrap();

    c.bench_function("gql/parameter_binding", |b| {
        b.iter(|| {
            for &age in &schedule {
                let mut params = HashMap::with_capacity(1);
                params.insert("age".to_string(), Value::Int(age));
                let r = engine
                    .execute_with_context(
                        black_box(&store),
                        GRAPH,
                        black_box(Q_PARAM),
                        &registry,
                        params,
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
