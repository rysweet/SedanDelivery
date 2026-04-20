//! Reliability bench — `sedan-gremlin-embedded` (Apache TinkerPop Gremlin).
//!
//! Mirrors the cypher/gql bench files. Gremlin does not expose the
//! `$param`-style binding API, so the `parameter_binding` slot here measures
//! the closest semantic equivalent: compile-once + execute-many on the same
//! traversal across 100 invocations. This is the steady-state cost users
//! see when re-running a hot traversal at runtime.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use embedded_bench::{seeded_store, GRAPH};
use sedan_gremlin_embedded::{GraphStore, GremlinEngine, InMemoryGraphStore};

const Q_SIMPLE_MATCH: &str = "g.V().hasLabel('Person').has('age', P.gt(21)).limit(10)";
const Q_TRAVERSAL_2HOP: &str = "g.V().out().out().limit(100)";
const Q_AGGREGATE: &str = "g.V().groupCount().by(label)";
const Q_PARAM_REPEAT: &str = "g.V().hasLabel('Person').has('age', 42).limit(5)";

fn bench_simple_match(c: &mut Criterion) {
    let store = seeded_store();
    let engine = GremlinEngine::new();
    engine.execute(&store, GRAPH, Q_SIMPLE_MATCH).unwrap();
    c.bench_function("gremlin/simple_match", |b| {
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
    let engine = GremlinEngine::new();
    engine.execute(&store, GRAPH, Q_TRAVERSAL_2HOP).unwrap();
    c.bench_function("gremlin/traversal_2hop", |b| {
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
    let engine = GremlinEngine::new();
    engine.execute(&store, GRAPH, Q_AGGREGATE).unwrap();
    c.bench_function("gremlin/aggregate", |b| {
        b.iter(|| {
            let r = engine
                .execute(black_box(&store), GRAPH, black_box(Q_AGGREGATE))
                .unwrap();
            black_box(r);
        });
    });
}

fn bench_mutation(c: &mut Criterion) {
    // Insert 100 fresh nodes per iteration. We use direct `GraphStore`
    // writes (not `g.addV(...)`) because `sedan-gremlin-embedded` is a
    // read-only traversal engine on top of `cypher-core`'s storage layer.
    // This still measures the underlying mutation hot path and keeps the
    // bench apples-to-apples with the cypher/gql variants.
    c.bench_function("gremlin/mutation", |b| {
        b.iter_with_setup(
            || {
                let store = InMemoryGraphStore::default();
                store.create_graph(GRAPH).unwrap();
                store
            },
            |store| {
                for i in 0..100 {
                    let mut props = indexmap::IndexMap::new();
                    props.insert("i".into(), sedan_gremlin_embedded::Value::Int(i as i64));
                    store.create_node(GRAPH, vec!["N".into()], props).unwrap();
                }
                black_box(&store);
            },
        );
    });
}

fn bench_parameter_binding(c: &mut Criterion) {
    // Gremlin's embedded surface has no `$param` binding API; the closest
    // analogue to "100 runs of the same query with different params" is
    // "100 runs of the same compiled traversal" — i.e. plan-cache reuse.
    let store = seeded_store();
    let engine = GremlinEngine::new();
    let compiled = engine.compile(Q_PARAM_REPEAT).unwrap();
    engine.execute_compiled(&store, GRAPH, &compiled).unwrap();

    c.bench_function("gremlin/parameter_binding", |b| {
        b.iter(|| {
            for _ in 0..100 {
                let r = engine
                    .execute_compiled(black_box(&store), GRAPH, &compiled)
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
