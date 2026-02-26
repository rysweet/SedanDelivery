// Parser benchmarks — Phase 1+.
use criterion::{criterion_group, criterion_main, Criterion};

fn bench_tokenize(c: &mut Criterion) {
    c.bench_function("tokenize_simple_query", |b| {
        b.iter(|| gql_core::parser::tokenize("MATCH (n:Person) WHERE n.age > 18 RETURN n"))
    });
}

criterion_group!(benches, bench_tokenize);
criterion_main!(benches);
