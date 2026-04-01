// Benchmarks comparing indexed vs unindexed property lookups on 10K nodes.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use cypher_core::storage::{GraphStore, InMemoryGraphStore, RangeQuery};
use cypher_core::types::Value;
use indexmap::IndexMap;

fn make_store(n: usize, indexed: bool) -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("bench").unwrap();
    if indexed {
        store.create_index("bench", "Person", "age").unwrap();
        store.create_index("bench", "Person", "name").unwrap();
    }
    for i in 0..n {
        let props = IndexMap::from([
            ("age".to_string(), Value::Int((i % 100) as i64)),
            (
                "name".to_string(),
                Value::Str(format!("person_{i:05}").into()),
            ),
        ]);
        store
            .create_node("bench", vec!["Person".to_string()], props)
            .unwrap();
    }
    store
}

fn bench_eq_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("property_index_eq");
    let n = 10_000;
    let store_unindexed = make_store(n, false);
    let store_indexed = make_store(n, true);
    let target = Value::Int(42);

    group.bench_with_input(BenchmarkId::new("unindexed", n), &n, |b, _| {
        b.iter(|| {
            store_unindexed
                .find_nodes_by_label_and_property("bench", "Person", "age", RangeQuery::Eq(&target))
                .unwrap()
        });
    });

    group.bench_with_input(BenchmarkId::new("indexed", n), &n, |b, _| {
        b.iter(|| {
            store_indexed
                .find_nodes_by_label_and_property("bench", "Person", "age", RangeQuery::Eq(&target))
                .unwrap()
        });
    });

    group.finish();
}

fn bench_range_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("property_index_range");
    let n = 10_000;
    let store_unindexed = make_store(n, false);
    let store_indexed = make_store(n, true);
    let lo = Value::Int(20);
    let hi = Value::Int(40);

    group.bench_with_input(BenchmarkId::new("unindexed", n), &n, |b, _| {
        b.iter(|| {
            store_unindexed
                .find_nodes_by_label_and_property(
                    "bench",
                    "Person",
                    "age",
                    RangeQuery::Between {
                        lo: &lo,
                        hi: &hi,
                        inclusive: true,
                    },
                )
                .unwrap()
        });
    });

    group.bench_with_input(BenchmarkId::new("indexed", n), &n, |b, _| {
        b.iter(|| {
            store_indexed
                .find_nodes_by_label_and_property(
                    "bench",
                    "Person",
                    "age",
                    RangeQuery::Between {
                        lo: &lo,
                        hi: &hi,
                        inclusive: true,
                    },
                )
                .unwrap()
        });
    });

    group.finish();
}

fn bench_starts_with_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("property_index_starts_with");
    let n = 10_000;
    let store_unindexed = make_store(n, false);
    let store_indexed = make_store(n, true);

    group.bench_with_input(BenchmarkId::new("unindexed", n), &n, |b, _| {
        b.iter(|| {
            store_unindexed
                .find_nodes_by_label_and_property(
                    "bench",
                    "Person",
                    "name",
                    RangeQuery::StartsWith("person_001"),
                )
                .unwrap()
        });
    });

    group.bench_with_input(BenchmarkId::new("indexed", n), &n, |b, _| {
        b.iter(|| {
            store_indexed
                .find_nodes_by_label_and_property(
                    "bench",
                    "Person",
                    "name",
                    RangeQuery::StartsWith("person_001"),
                )
                .unwrap()
        });
    });

    group.finish();
}

fn bench_gt_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("property_index_gt");
    let n = 10_000;
    let store_unindexed = make_store(n, false);
    let store_indexed = make_store(n, true);
    let bound = Value::Int(90);

    group.bench_with_input(BenchmarkId::new("unindexed", n), &n, |b, _| {
        b.iter(|| {
            store_unindexed
                .find_nodes_by_label_and_property("bench", "Person", "age", RangeQuery::Gt(&bound))
                .unwrap()
        });
    });

    group.bench_with_input(BenchmarkId::new("indexed", n), &n, |b, _| {
        b.iter(|| {
            store_indexed
                .find_nodes_by_label_and_property("bench", "Person", "age", RangeQuery::Gt(&bound))
                .unwrap()
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_eq_lookup,
    bench_range_lookup,
    bench_starts_with_lookup,
    bench_gt_lookup,
);
criterion_main!(benches);
