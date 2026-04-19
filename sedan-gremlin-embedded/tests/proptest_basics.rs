//! Property-based small-graph invariants for the embedded Gremlin engine.
//!
//! Generates a tiny random graph (≤8 nodes, ≤8 edges) directly through the
//! [`GraphStore`] API, then asserts that `g.V().count()` and `g.E().count()`
//! agree with the seeded counts. Counterexamples shrink to the smallest
//! graph that violates the invariant.

use cypher_core::storage::{GraphStore, InMemoryGraphStore};
use cypher_core::types::Value;
use indexmap::IndexMap;
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::test_runner::Config;
use sedan_gremlin_embedded::GremlinEngine;

#[derive(Debug, Clone)]
struct EdgeSpec {
    src_idx: usize,
    dst_idx: usize,
    rel: String,
}

fn rel_strategy() -> impl Strategy<Value = String> {
    prop::sample::select(vec!["knows", "likes", "follows"]).prop_map(|s| s.into())
}

fn arb_graph() -> impl Strategy<Value = (usize, Vec<EdgeSpec>)> {
    (1usize..=8).prop_flat_map(|n| {
        let edges = vec(
            (0..n, 0..n, rel_strategy()).prop_map(|(s, d, r)| EdgeSpec {
                src_idx: s,
                dst_idx: d,
                rel: r,
            }),
            0..=8,
        );
        (Just(n), edges)
    })
}

fn extract_count(rows: &[std::collections::HashMap<String, Value>]) -> i64 {
    assert_eq!(rows.len(), 1, "count() must return one row, got {rows:?}");
    let row = &rows[0];
    let v = row.values().next().expect("count row has one column");
    match v {
        Value::Int(n) => *n,
        other => panic!("expected Int count, got {other:?}"),
    }
}

proptest! {
    #![proptest_config(Config { cases: 64, .. Config::default() })]

    #[test]
    fn vertex_and_edge_counts_match_seeded((n_nodes, edges) in arb_graph()) {
        let store = InMemoryGraphStore::default();
        store.create_graph("g").unwrap();

        let mut node_ids = Vec::with_capacity(n_nodes);
        for _ in 0..n_nodes {
            node_ids.push(
                store
                    .create_node("g", vec!["V".into()], IndexMap::new())
                    .unwrap(),
            );
        }
        for e in &edges {
            store
                .create_edge(
                    "g",
                    node_ids[e.src_idx],
                    node_ids[e.dst_idx],
                    e.rel.clone(),
                    IndexMap::new(),
                )
                .unwrap();
        }

        let engine = GremlinEngine::new();
        let v_rows = engine
            .execute(&store as &dyn GraphStore, "g", "g.V().count()")
            .unwrap()
            .rows;
        let e_rows = engine
            .execute(&store as &dyn GraphStore, "g", "g.E().count()")
            .unwrap()
            .rows;

        prop_assert_eq!(extract_count(&v_rows), n_nodes as i64);
        prop_assert_eq!(extract_count(&e_rows), edges.len() as i64);
    }
}
