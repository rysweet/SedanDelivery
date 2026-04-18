use criterion::{criterion_group, criterion_main, Criterion};
use indexmap::IndexMap;
use sedan_gremlin_embedded::{GraphStore, GremlinEngine, InMemoryGraphStore, Value};
use std::hint::black_box;

fn seeded_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();
    let mut node_ids = Vec::new();
    for i in 0..100 {
        let nid = store
            .create_node(
                "g",
                vec!["Person".into()],
                IndexMap::from([
                    ("name".into(), Value::Str(format!("person_{i}").into())),
                    ("age".into(), Value::Int(20 + i)),
                ]),
            )
            .unwrap();
        node_ids.push(nid);
    }
    // Add some edges: person_i -> person_(i+1)
    for i in 0..99 {
        store
            .create_edge(
                "g",
                node_ids[i],
                node_ids[i + 1],
                "knows".to_string(),
                IndexMap::new(),
            )
            .unwrap();
    }
    store
}

fn bench_execute(c: &mut Criterion) {
    let engine = GremlinEngine::new();
    let store = seeded_store();

    let mut group = c.benchmark_group("execute");

    group.bench_function("count", |b| {
        b.iter(|| {
            engine
                .execute(&store, "g", black_box("g.V().count()"))
                .unwrap()
        });
    });

    group.bench_function("has_filter", |b| {
        b.iter(|| {
            engine
                .execute(
                    &store,
                    "g",
                    black_box(r#"g.V().has("name","person_50").values("name")"#),
                )
                .unwrap()
        });
    });

    group.bench_function("traversal", |b| {
        b.iter(|| {
            engine
                .execute(
                    &store,
                    "g",
                    black_box(r#"g.V().has("name","person_0").out("knows").values("name")"#),
                )
                .unwrap()
        });
    });

    group.bench_function("aggregation", |b| {
        b.iter(|| {
            engine
                .execute(&store, "g", black_box(r#"g.V().values("age").sum()"#))
                .unwrap()
        });
    });

    group.bench_function("limit", |b| {
        b.iter(|| {
            engine
                .execute(&store, "g", black_box("g.V().limit(10).count()"))
                .unwrap()
        });
    });

    group.finish();
}

fn bench_compiled(c: &mut Criterion) {
    let engine = GremlinEngine::new();
    let store = seeded_store();

    let mut group = c.benchmark_group("compiled");

    // Pre-compile queries once
    let count_q = engine.compile("g.V().count()").unwrap();
    let filter_q = engine
        .compile(r#"g.V().has("name","person_50").values("name")"#)
        .unwrap();
    let traversal_q = engine
        .compile(r#"g.V().has("name","person_0").out("knows").values("name")"#)
        .unwrap();
    let agg_q = engine.compile(r#"g.V().values("age").sum()"#).unwrap();

    group.bench_function("count", |b| {
        b.iter(|| {
            engine
                .execute_compiled(&store, "g", black_box(&count_q))
                .unwrap()
        });
    });

    group.bench_function("has_filter", |b| {
        b.iter(|| {
            engine
                .execute_compiled(&store, "g", black_box(&filter_q))
                .unwrap()
        });
    });

    group.bench_function("traversal", |b| {
        b.iter(|| {
            engine
                .execute_compiled(&store, "g", black_box(&traversal_q))
                .unwrap()
        });
    });

    group.bench_function("aggregation", |b| {
        b.iter(|| {
            engine
                .execute_compiled(&store, "g", black_box(&agg_q))
                .unwrap()
        });
    });

    group.finish();
}

fn bench_compile_only(c: &mut Criterion) {
    let engine = GremlinEngine::new();

    let mut group = c.benchmark_group("compile");

    group.bench_function("count", |b| {
        b.iter(|| engine.compile(black_box("g.V().count()")).unwrap());
    });

    group.bench_function("complex_traversal", |b| {
        b.iter(|| {
            engine
                .compile(black_box(
                    r#"g.V().has("name","person_0").out("knows").values("name")"#,
                ))
                .unwrap()
        });
    });

    group.finish();
}

criterion_group!(benches, bench_execute, bench_compiled, bench_compile_only);
criterion_main!(benches);
