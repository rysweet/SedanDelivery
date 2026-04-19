//! High-signal embedded benchmark suite for `sedan-gremlin-embedded`.
//!
//! Measures the headline scenarios users care about when embedding the
//! Gremlin engine in-process:
//!   (a) `g.V().count()` over a 100-node graph
//!   (b) `g.V().out().limit(50).toList()` over 100 nodes / ~200 edges
//!       (filtered traversal with early termination)
//!   (c) compiled-traversal-reuse cost: compile once, execute many times
//!       vs. parse+compile+execute on every call
//!
//! See `docs/PERFORMANCE.md` for how to run / interpret these benchmarks.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use indexmap::IndexMap;
use sedan_gremlin_embedded::{GraphStore, GremlinEngine, InMemoryGraphStore, Value};

const NODES: usize = 100;
const EXTRA_EDGES: usize = 100;

fn seeded() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();
    let mut ids = Vec::with_capacity(NODES);
    for i in 0..NODES {
        let id = store
            .create_node(
                "g",
                vec!["Person".into()],
                IndexMap::from([
                    ("id".into(), Value::Int(i as i64)),
                    ("name".into(), Value::Str(format!("p_{i}").into())),
                ]),
            )
            .unwrap();
        ids.push(id);
    }
    // Chain edges p_i -[:knows]-> p_{i+1}.
    for i in 0..(NODES - 1) {
        store
            .create_edge("g", ids[i], ids[i + 1], "knows".into(), IndexMap::new())
            .unwrap();
    }
    // Extra fan-out edges.
    for i in 0..EXTRA_EDGES {
        let a = i % NODES;
        let b = (i * 7 + 3) % NODES;
        if a == b {
            continue;
        }
        store
            .create_edge("g", ids[a], ids[b], "knows".into(), IndexMap::new())
            .unwrap();
    }
    store
}

fn bench_count(c: &mut Criterion) {
    let store = seeded();
    let engine = GremlinEngine::new();
    engine.execute(&store, "g", "g.V().count()").unwrap();

    c.bench_function("gremlin/v_count", |b| {
        b.iter(|| {
            let r = engine
                .execute(black_box(&store), "g", black_box("g.V().count()"))
                .unwrap();
            black_box(r);
        });
    });
}

fn bench_out_limit(c: &mut Criterion) {
    let store = seeded();
    let engine = GremlinEngine::new();
    let q = "g.V().out().limit(50)";
    engine.execute(&store, "g", q).unwrap();

    c.bench_function("gremlin/v_out_limit_50", |b| {
        b.iter(|| {
            let r = engine
                .execute(black_box(&store), "g", black_box(q))
                .unwrap();
            black_box(r);
        });
    });
}

fn bench_compile_reuse(c: &mut Criterion) {
    let mut group = c.benchmark_group("gremlin/compile_reuse");
    let store = seeded();
    let engine = GremlinEngine::new();
    let q = "g.V().has('id', 5).out().limit(10)";

    // Compile-on-every-call: full parse + compile + execute path.
    group.bench_function("compile_each_call", |b| {
        b.iter(|| {
            let r = engine
                .execute(black_box(&store), "g", black_box(q))
                .unwrap();
            black_box(r);
        });
    });

    // Compile-once-execute-many: amortise parser/compiler cost.
    let compiled = engine.compile(q).unwrap();
    group.bench_function("compile_once", |b| {
        b.iter(|| {
            let r = engine
                .execute_compiled(black_box(&store), "g", black_box(&compiled))
                .unwrap();
            black_box(r);
        });
    });

    group.finish();
}

criterion_group!(embedded, bench_count, bench_out_limit, bench_compile_reuse,);
criterion_main!(embedded);
