use criterion::{criterion_group, criterion_main, Criterion};
use gremlin_core::types::GremlinValue;
use gremlin_core::{parse_traversal, GremlinCompiler};
use std::hint::black_box;

fn no_params(_: &str) -> Option<GremlinValue> {
    None
}

fn bench_parse(c: &mut Criterion) {
    let queries = [
        ("simple_count", "g.V().count()"),
        ("has_filter", r#"g.V().has("name","Alice").values("name")"#),
        (
            "traversal_chain",
            r#"g.V().hasLabel("Person").out("knows").out("created").values("name")"#,
        ),
        (
            "mutation",
            r#"g.addV("Person").property("name","Alice").property("age",30)"#,
        ),
        (
            "aggregate",
            r#"g.V().hasLabel("Person").values("age").order().dedup().limit(10)"#,
        ),
    ];

    let mut group = c.benchmark_group("parse");
    for (name, query) in &queries {
        group.bench_function(*name, |b| {
            b.iter(|| parse_traversal(black_box(query), &no_params).unwrap());
        });
    }
    group.finish();
}

fn bench_compile(c: &mut Criterion) {
    let queries = [
        ("simple_count", "g.V().count()"),
        ("has_filter", r#"g.V().has("name","Alice").values("name")"#),
        (
            "traversal_chain",
            r#"g.V().hasLabel("Person").out("knows").out("created").values("name")"#,
        ),
    ];

    let mut group = c.benchmark_group("compile");
    for (name, query) in &queries {
        let steps = parse_traversal(query, &no_params).unwrap();
        group.bench_function(*name, |b| {
            b.iter(|| {
                let mut compiler = GremlinCompiler::new();
                compiler.compile(black_box(&steps)).unwrap();
            });
        });
    }
    group.finish();
}

fn bench_parse_and_compile(c: &mut Criterion) {
    let query = r#"g.V().hasLabel("Person").has("age",P.gt(25)).out("knows").values("name").dedup().limit(10)"#;

    c.bench_function("end_to_end", |b| {
        b.iter(|| {
            let steps = parse_traversal(black_box(query), &no_params).unwrap();
            let mut compiler = GremlinCompiler::new();
            compiler.compile(&steps).unwrap();
        });
    });
}

criterion_group!(benches, bench_parse, bench_compile, bench_parse_and_compile);
criterion_main!(benches);
