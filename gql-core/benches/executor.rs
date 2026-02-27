// Executor benchmarks — Phase 4 (issue #189): comprehensive performance baselines.
//
// Covers: node scan, label scan, filter, sort, aggregation, graph expansion,
// end-to-end social queries, and CREATE throughput.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use gql_core::executor::{ExecutionBudget, QueryExecutor};
use gql_core::parser::parse_query;
use gql_core::planner::QueryPlanner;
use gql_core::storage::{GraphStore, InMemoryGraphStore};
use gql_core::types::Value;
use std::collections::HashMap;
use std::time::Duration;

// ── Setup helpers ──────────────────────────────────────────────────────────────

fn make_person_store(n: usize) -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("bench").unwrap();
    for i in 0..n {
        let props = HashMap::from([
            ("name".to_string(), Value::Str(format!("person_{i}").into())),
            ("age".to_string(), Value::Int((i % 100) as i64)),
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

// ── §4.2 Node / Label Scan ───────────────────────────────────────────────────

fn bench_node_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("node_scan");
    for size in [100usize, 1_000, 10_000] {
        let store = make_person_store(size);
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| run_query(&store, "MATCH (n:Person) RETURN count(n)"));
        });
    }
    group.finish();
}

// ── §4.2 Filter ──────────────────────────────────────────────────────────────

fn bench_filter_equality(c: &mut Criterion) {
    let store = make_person_store(1_000);
    c.bench_function("filter_equality_1k", |b| {
        b.iter(|| run_query(&store, "MATCH (n:Person) WHERE n.age = 42 RETURN count(n)"));
    });
}

fn bench_filter_range(c: &mut Criterion) {
    let store = make_person_store(1_000);
    c.bench_function("filter_range_1k", |b| {
        b.iter(|| run_query(&store, "MATCH (n:Person) WHERE n.age > 50 RETURN count(n)"));
    });
}

fn bench_filter_string_prefix(c: &mut Criterion) {
    let store = make_person_store(1_000);
    c.bench_function("filter_starts_with_1k", |b| {
        b.iter(|| {
            run_query(
                &store,
                "MATCH (n:Person) WHERE n.name STARTS WITH 'person_1' RETURN count(n)",
            )
        });
    });
}

// ── §4.2 Sort ────────────────────────────────────────────────────────────────

fn bench_sort(c: &mut Criterion) {
    let mut group = c.benchmark_group("sort");
    for size in [100usize, 1_000] {
        let store = make_person_store(size);
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| run_query(&store, "MATCH (n:Person) RETURN n.name ORDER BY n.name"));
        });
    }
    group.finish();
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

// ── §4.2 Graph Expansion ─────────────────────────────────────────────────────

fn bench_expand_star_graph(c: &mut Criterion) {
    // 1 hub → 1 000 leaves.
    let store = InMemoryGraphStore::default();
    store.create_graph("bench").unwrap();
    let hub = store
        .create_node("bench", vec!["Hub".to_string()], HashMap::new())
        .unwrap();
    for i in 0..1_000i64 {
        let leaf = store
            .create_node(
                "bench",
                vec!["Leaf".to_string()],
                HashMap::from([("i".to_string(), Value::Int(i))]),
            )
            .unwrap();
        store
            .create_edge("bench", hub, leaf, "LINK".to_string(), HashMap::new())
            .unwrap();
    }

    c.bench_function("expand_star_1000", |b| {
        b.iter(|| run_query(&store, "MATCH (h:Hub)-[:LINK]->(l:Leaf) RETURN count(l)"));
    });
}

fn bench_expand_chain(c: &mut Criterion) {
    // 500-node chain: 0→1→2→…→499.
    let store = InMemoryGraphStore::default();
    store.create_graph("bench").unwrap();
    let mut prev = store
        .create_node("bench", vec!["Ch".to_string()], HashMap::new())
        .unwrap();
    for _ in 0..499 {
        let next = store
            .create_node("bench", vec!["Ch".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("bench", prev, next, "N".to_string(), HashMap::new())
            .unwrap();
        prev = next;
    }

    c.bench_function("expand_chain_500", |b| {
        b.iter(|| run_query(&store, "MATCH (a:Ch)-[:N]->(b:Ch) RETURN count(*)"));
    });
}

// ── §4.3 End-to-End Social Queries ───────────────────────────────────────────

fn bench_e2e_friends_of_friends(c: &mut Criterion) {
    // 100 persons, each connected to the next 5 neighbours (ring topology).
    let store = InMemoryGraphStore::default();
    store.create_graph("bench").unwrap();
    let mut ids = vec![];
    for i in 0..100i64 {
        ids.push(
            store
                .create_node(
                    "bench",
                    vec!["Person".to_string()],
                    HashMap::from([("i".to_string(), Value::Int(i))]),
                )
                .unwrap(),
        );
    }
    for i in 0..100usize {
        for j in 1..=5 {
            let dst = ids[(i + j) % 100];
            store
                .create_edge("bench", ids[i], dst, "KNOWS".to_string(), HashMap::new())
                .unwrap();
        }
    }

    c.bench_function("e2e_friends_of_friends", |b| {
        b.iter(|| {
            run_query(
                &store,
                "MATCH (a:Person)-[:KNOWS]->(b:Person)-[:KNOWS]->(c:Person) \
                 RETURN count(DISTINCT c)",
            )
        });
    });
}

fn bench_e2e_full_text_like(c: &mut Criterion) {
    let store = make_person_store(1_000);
    c.bench_function("e2e_starts_with_1k", |b| {
        b.iter(|| {
            run_query(
                &store,
                "MATCH (n:Person) WHERE n.name STARTS WITH 'person_5' RETURN n.name ORDER BY n.name",
            )
        });
    });
}

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

// ── §4.2 CREATE Throughput ───────────────────────────────────────────────────

fn bench_create_node_throughput(c: &mut Criterion) {
    c.bench_function("create_100_nodes", |b| {
        b.iter(|| {
            let store = InMemoryGraphStore::default();
            store.create_graph("bench").unwrap();
            run_query(&store, "UNWIND range(1, 100) AS i CREATE (:Node {i: i})");
        });
    });
}

criterion_group!(
    benches,
    bench_node_scan,
    bench_filter_equality,
    bench_filter_range,
    bench_filter_string_prefix,
    bench_sort,
    bench_aggregate_count_group_by,
    bench_expand_star_graph,
    bench_expand_chain,
    bench_e2e_friends_of_friends,
    bench_e2e_full_text_like,
    bench_e2e_recommendation,
    bench_create_node_throughput,
);
criterion_main!(benches);
