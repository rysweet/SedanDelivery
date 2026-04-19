// Executor benchmarks — graph expansion, traversal, variable-length paths,
// and end-to-end social queries. Decomposed from benches/executor.rs (issue #1664).

use criterion::{criterion_group, criterion_main, Criterion};
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

fn run_query(store: &InMemoryGraphStore, cypher: &str) {
    let ast = parse_query(cypher).unwrap();
    let planner = QueryPlanner::new(store, "bench");
    let plan = planner.plan(&ast).unwrap();
    let executor = QueryExecutor::new(store);
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), 50_000_000);
    executor.execute("bench", &plan, &mut budget).unwrap();
}

// ── §4.2 Graph Expansion ─────────────────────────────────────────────────────

fn bench_expand_star_graph(c: &mut Criterion) {
    // 1 hub → 1 000 leaves.
    let store = InMemoryGraphStore::default();
    store.create_graph("bench").unwrap();
    let hub = store
        .create_node("bench", vec!["Hub".to_string()], IndexMap::new())
        .unwrap();
    for i in 0..1_000i64 {
        let leaf = store
            .create_node(
                "bench",
                vec!["Leaf".to_string()],
                IndexMap::from([("i".to_string(), Value::Int(i))]),
            )
            .unwrap();
        store
            .create_edge("bench", hub, leaf, "LINK".to_string(), IndexMap::new())
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
        .create_node("bench", vec!["Ch".to_string()], IndexMap::new())
        .unwrap();
    for _ in 0..499 {
        let next = store
            .create_node("bench", vec!["Ch".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_edge("bench", prev, next, "N".to_string(), IndexMap::new())
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
                    IndexMap::from([("i".to_string(), Value::Int(i))]),
                )
                .unwrap(),
        );
    }
    for i in 0..100usize {
        for j in 1..=5 {
            let dst = ids[(i + j) % 100];
            store
                .create_edge("bench", ids[i], dst, "KNOWS".to_string(), IndexMap::new())
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

// ── §4.4 Variable-Length Paths ───────────────────────────────────────────────

fn bench_varlen_paths(c: &mut Criterion) {
    // 100-node chain: 0→1→2→…→99. Query paths of length 1..3.
    let store = InMemoryGraphStore::default();
    store.create_graph("bench").unwrap();
    let mut prev = store
        .create_node("bench", vec!["VL".to_string()], IndexMap::new())
        .unwrap();
    for _ in 0..99 {
        let next = store
            .create_node("bench", vec!["VL".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_edge("bench", prev, next, "NEXT".to_string(), IndexMap::new())
            .unwrap();
        prev = next;
    }

    c.bench_function("varlen_1to3_chain100", |b| {
        b.iter(|| run_query(&store, "MATCH (a:VL)-[:NEXT*1..3]->(b:VL) RETURN count(*)"));
    });
}

criterion_group!(
    benches,
    bench_expand_star_graph,
    bench_expand_chain,
    bench_e2e_friends_of_friends,
    bench_e2e_full_text_like,
    bench_varlen_paths,
);
criterion_main!(benches);
