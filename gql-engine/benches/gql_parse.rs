//! Benchmarks for GQL parsing, rewriting, and end-to-end execution.

use criterion::{criterion_group, criterion_main, Criterion};
use gql_engine::parser::{parse_gql, rewrite};
use gql_engine::{execute, GraphStore, InMemoryGraphStore};
use std::hint::black_box;

// ── Normalisation benchmarks ────────────────────────────────────────────────

fn bench_normalise_simple(c: &mut Criterion) {
    let src = "MATCH (n:Person) WHERE n.age > 21 RETURN n.name";
    c.bench_function("normalise_simple_cypher", |b| {
        b.iter(|| rewrite::normalise_gql_source(black_box(src)))
    });
}

fn bench_normalise_gql_keywords(c: &mut Criterion) {
    let src = "INSERT (n:Person {name: 'Alice'}) LET x = n.name FILTER x = 'Alice' RETURN x";
    c.bench_function("normalise_gql_keywords", |b| {
        b.iter(|| rewrite::normalise_gql_source(black_box(src)))
    });
}

fn bench_normalise_mandatory_match(c: &mut Criterion) {
    let src = "MANDATORY MATCH (n:Person) RETURN n.name";
    c.bench_function("normalise_mandatory_match", |b| {
        b.iter(|| rewrite::normalise_gql_source(black_box(src)))
    });
}

fn bench_extract_metadata(c: &mut Criterion) {
    let src = "MANDATORY MATCH TRAIL ANY SHORTEST (a)-[*]->(b) RETURN a";
    c.bench_function("extract_gql_metadata", |b| {
        b.iter(|| rewrite::extract_gql_metadata(black_box(src)))
    });
}

// ── Parse benchmarks ────────────────────────────────────────────────────────

fn bench_parse_gql_simple(c: &mut Criterion) {
    let src = "MATCH (n:Person) WHERE n.age > 21 RETURN n.name";
    c.bench_function("parse_gql_simple", |b| b.iter(|| parse_gql(black_box(src))));
}

fn bench_parse_gql_insert(c: &mut Criterion) {
    let src = "INSERT (n:Person {name: 'Alice', age: 30}) RETURN n.name";
    c.bench_function("parse_gql_insert", |b| b.iter(|| parse_gql(black_box(src))));
}

fn bench_parse_gql_complex(c: &mut Criterion) {
    let src = "MATCH (a:Person)-[:KNOWS]->(b:Person)-[:LIVES_IN]->(c:City) \
               FILTER a.age > 25 AND b.age < 40 \
               RETURN a.name, b.name, c.name ORDER BY a.name LIMIT 10";
    c.bench_function("parse_gql_complex", |b| {
        b.iter(|| parse_gql(black_box(src)))
    });
}

// ── End-to-end execute benchmarks ───────────────────────────────────────────

fn bench_execute_return_literal(c: &mut Criterion) {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();

    c.bench_function("execute_return_literal", |b| {
        b.iter(|| execute(black_box("RETURN 1 + 2 AS x"), &store, "g"))
    });
}

fn bench_execute_insert_and_match(c: &mut Criterion) {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();

    // Pre-populate so MATCH has data to scan
    for i in 0..100 {
        execute(
            &format!("CREATE (n:Person {{name: 'p{i}', age: {i}}})"),
            &store,
            "g",
        )
        .unwrap();
    }

    c.bench_function("execute_match_filter_100_nodes", |b| {
        b.iter(|| {
            execute(
                black_box("MATCH (n:Person) FILTER n.age > 50 RETURN n.name"),
                &store,
                "g",
            )
        })
    });
}

fn bench_execute_for_unwind(c: &mut Criterion) {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();

    c.bench_function("execute_for_unwind_list", |b| {
        b.iter(|| {
            execute(
                black_box("FOR x IN [1,2,3,4,5,6,7,8,9,10] RETURN x"),
                &store,
                "g",
            )
        })
    });
}

criterion_group!(
    benches,
    bench_normalise_simple,
    bench_normalise_gql_keywords,
    bench_normalise_mandatory_match,
    bench_extract_metadata,
    bench_parse_gql_simple,
    bench_parse_gql_insert,
    bench_parse_gql_complex,
    bench_execute_return_literal,
    bench_execute_insert_and_match,
    bench_execute_for_unwind,
);
criterion_main!(benches);
