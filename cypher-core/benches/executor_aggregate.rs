// Executor benchmarks — aggregation and property cache.
// Decomposed from benches/executor.rs (issue #1664).

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use cypher_core::executor::{ExecutionBudget, QueryExecutor};
use cypher_core::parser::parse_query;
use cypher_core::planner::QueryPlanner;
use cypher_core::storage::{GraphStore, InMemoryGraphStore};
use cypher_core::types::Value;
use indexmap::IndexMap;
use std::time::Duration;

fn make_person_store(n: usize) -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("bench").unwrap();
    for i in 0..n {
        let props = IndexMap::from([
            ("name".to_string(), Value::Str(format!("person_{i}").into())),
            ("age".to_string(), Value::Int((i % 100) as i64)),
        ]);
        store
            .create_node("bench", vec!["Person".to_string()], props)
            .unwrap();
    }
    store
}

fn make_rich_person_store(n: usize) -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("bench").unwrap();
    for i in 0..n {
        let props = IndexMap::from([
            ("name".to_string(), Value::Str(format!("person_{i}").into())),
            ("age".to_string(), Value::Int((i % 100) as i64)),
            (
                "email".to_string(),
                Value::Str(format!("person_{i}@example.com").into()),
            ),
            (
                "city".to_string(),
                Value::Str(format!("city_{}", i % 50).into()),
            ),
            ("score".to_string(), Value::Float((i as f64) * 1.5)),
        ]);
        store
            .create_node("bench", vec!["Person".to_string()], props)
            .unwrap();
    }
    store
}

fn run_query(store: &InMemoryGraphStore, cypher: &str) {
    let ast = parse_query(cypher).unwrap();
    let planner = QueryPlanner::new(store, "bench");
    let plan = planner.plan(&ast).unwrap();
    let executor = QueryExecutor::new(store);
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), 50_000_000);
    executor.execute("bench", &plan, &mut budget).unwrap();
}

// ── §4.2 Aggregation ─────────────────────────────────────────────────────────

fn bench_aggregate_count_group_by(c: &mut Criterion) {
    let store = make_person_store(1_000);
    c.bench_function("aggregate_count_group_by_1k", |b| {
        b.iter(|| {
            run_query(
                &store,
                "MATCH (n:Person) RETURN n.age, count(n) ORDER BY n.age",
            )
        });
    });
}

// ── §4.4 Aggregation with Many Groups ───────────────────────────────────────

fn bench_aggregate_many_groups(c: &mut Criterion) {
    let store = InMemoryGraphStore::default();
    store.create_graph("bench").unwrap();
    for i in 0..10_000usize {
        let props = IndexMap::from([
            (
                "group".to_string(),
                Value::Str(format!("g_{}", i % 1_000).into()),
            ),
            ("val".to_string(), Value::Int(i as i64)),
        ]);
        store
            .create_node("bench", vec!["Item".to_string()], props)
            .unwrap();
    }

    c.bench_function("aggregate_1000_groups_10k", |b| {
        b.iter(|| run_query(&store, "MATCH (n:Item) RETURN n.group, count(*)"));
    });
}

// ── §4.3 Recommendation (aggregation + ORDER BY + LIMIT) ─────────────────────

fn bench_e2e_recommendation(c: &mut Criterion) {
    let store = make_person_store(200);
    c.bench_function("e2e_recommendation_200", |b| {
        b.iter(|| {
            run_query(
                &store,
                "MATCH (n:Person) RETURN n.age AS age, count(n) AS cnt \
                 ORDER BY cnt DESC LIMIT 10",
            )
        });
    });
}

// ── §4.4 Property Cache ─────────────────────────────────────────────────────

fn bench_multi_property_access(c: &mut Criterion) {
    let mut group = c.benchmark_group("property_cache");
    for size in [100usize, 1_000] {
        let store = make_rich_person_store(size);
        group.bench_with_input(BenchmarkId::new("multi_prop_5", size), &size, |b, _| {
            b.iter(|| {
                run_query(
                    &store,
                    "MATCH (n:Person) RETURN n.name, n.age, n.email, n.city, n.score",
                )
            });
        });
        group.bench_with_input(BenchmarkId::new("single_prop", size), &size, |b, _| {
            b.iter(|| run_query(&store, "MATCH (n:Person) RETURN n.name"));
        });
    }
    group.finish();
}

// ── §4.4 Multi-Property Access (Wide) ───────────────────────────────────────

fn bench_wide_multi_prop_access(c: &mut Criterion) {
    let store = InMemoryGraphStore::default();
    store.create_graph("bench").unwrap();
    for i in 0..10_000i64 {
        let props = IndexMap::from([
            ("a".to_string(), Value::Int(i)),
            ("b".to_string(), Value::Int(i * 2)),
            ("c".to_string(), Value::Str(format!("c_{i}").into())),
            ("d".to_string(), Value::Bool(i % 2 == 0)),
            ("e".to_string(), Value::Int(i * 10)),
        ]);
        store
            .create_node("bench", vec!["Wide".to_string()], props)
            .unwrap();
    }

    c.bench_function("multi_prop_5cols_10k", |b| {
        b.iter(|| run_query(&store, "MATCH (n:Wide) RETURN n.a, n.b, n.c, n.d, n.e"));
    });
}

criterion_group!(
    benches,
    bench_aggregate_count_group_by,
    bench_aggregate_many_groups,
    bench_e2e_recommendation,
    bench_multi_property_access,
    bench_wide_multi_prop_access,
);
criterion_main!(benches);
