// Parser benchmarks — Phase 4 (issue #189): tokeniser and parser throughput.
//
// Covers: simple queries, complex multi-clause queries, CREATE statements,
// and deeply-nested expressions.

use criterion::{criterion_group, criterion_main, Criterion};
use cypher_core::parser::{parse_query, tokenize};

// ── Existing benchmark (kept for baseline continuity) ─────────────────────────

fn bench_tokenize(c: &mut Criterion) {
    c.bench_function("tokenize_simple_query", |b| {
        b.iter(|| tokenize("MATCH (n:Person) WHERE n.age > 18 RETURN n"))
    });
}

// ── §4.1 Additional Parser Benchmarks ────────────────────────────────────────

fn bench_parse_simple_match_return(c: &mut Criterion) {
    c.bench_function("parse_simple_match_return", |b| {
        b.iter(|| {
            parse_query("MATCH (n:Person) WHERE n.age > 18 RETURN n.name ORDER BY n.name LIMIT 10")
        })
    });
}

fn bench_parse_complex_multi_clause(c: &mut Criterion) {
    let query = "MATCH (a:Person)-[:KNOWS]->(b:Person) \
                 WHERE a.age > 20 AND b.name STARTS WITH 'A' \
                 WITH a, collect(b.name) AS friends \
                 ORDER BY size(friends) DESC \
                 RETURN a.name, friends \
                 LIMIT 25";
    c.bench_function("parse_complex_multi_clause", |b| {
        b.iter(|| parse_query(query))
    });
}

fn bench_parse_create_statement(c: &mut Criterion) {
    let query = "CREATE (a:Person {name:'Alice',age:30})-[:KNOWS]->(b:Person {name:'Bob',age:25})";
    c.bench_function("parse_create_statement", |b| b.iter(|| parse_query(query)));
}

fn bench_parse_deeply_nested_expression(c: &mut Criterion) {
    // 50 levels of nested arithmetic: 1+(1+(1+…))
    let inner: String = (0..50).fold("1".to_string(), |acc, _| format!("1 + ({acc})"));
    let query = format!("RETURN {inner}");
    c.bench_function("parse_deeply_nested_50", |b| b.iter(|| parse_query(&query)));
}

fn bench_tokenize_throughput(c: &mut Criterion) {
    // Measure raw tokeniser throughput in bytes/sec with a long query.
    let long_query =
        "MATCH (n:Person) WHERE n.age > 18 RETURN n.name, n.age ORDER BY n.age DESC LIMIT 100 "
            .repeat(20);
    c.bench_function("tokenize_throughput", |b| b.iter(|| tokenize(&long_query)));
}

fn bench_parse_merge_with_on_clauses(c: &mut Criterion) {
    let query = "MERGE (n:Person {name:'Alice'}) \
                 ON CREATE SET n.created = 1, n.score = 0 \
                 ON MATCH SET n.score = n.score + 1 \
                 RETURN n";
    c.bench_function("parse_merge_with_on_clauses", |b| {
        b.iter(|| parse_query(query))
    });
}

criterion_group!(
    benches,
    bench_tokenize,
    bench_parse_simple_match_return,
    bench_parse_complex_multi_clause,
    bench_parse_create_statement,
    bench_parse_deeply_nested_expression,
    bench_tokenize_throughput,
    bench_parse_merge_with_on_clauses,
);
criterion_main!(benches);
