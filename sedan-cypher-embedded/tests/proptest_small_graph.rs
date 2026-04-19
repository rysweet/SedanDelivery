//! Property-based small-graph invariants for the embedded Cypher engine.
//!
//! Generates a tiny random graph (≤8 nodes, ≤8 edges) directly through the
//! [`GraphStore`] API, then asserts that `MATCH (n) RETURN count(n)` and
//! `MATCH ()-[r]->() RETURN count(r)` agree with the seeded counts.
//! Counterexamples shrink to the smallest graph that violates the invariant.

use indexmap::IndexMap;
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::test_runner::Config;
use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, Value};

#[derive(Debug, Clone)]
struct NodeSpec {
    label: Option<String>,
    prop: Option<(String, i64)>,
}

#[derive(Debug, Clone)]
struct EdgeSpec {
    src_idx: usize,
    dst_idx: usize,
    rel: String,
}

fn label_strategy() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(None),
        prop::sample::select(vec!["A", "B", "C"]).prop_map(|s| Some(s.into()))
    ]
}

fn rel_strategy() -> impl Strategy<Value = String> {
    prop::sample::select(vec!["KNOWS", "LIKES", "FOLLOWS"]).prop_map(|s| s.into())
}

fn arb_node() -> impl Strategy<Value = NodeSpec> {
    (
        label_strategy(),
        prop::option::of((
            "[a-z]{1,4}".prop_map(String::from),
            any::<i32>().prop_map(|n| n as i64),
        )),
    )
        .prop_map(|(label, prop)| NodeSpec { label, prop })
}

fn arb_graph() -> impl Strategy<Value = (Vec<NodeSpec>, Vec<EdgeSpec>)> {
    vec(arb_node(), 1..=8).prop_flat_map(|nodes| {
        let n = nodes.len();
        let edges = vec(
            (0..n, 0..n, rel_strategy()).prop_map(|(s, d, r)| EdgeSpec {
                src_idx: s,
                dst_idx: d,
                rel: r,
            }),
            0..=8,
        );
        (Just(nodes), edges)
    })
}

fn count_value(v: &Value) -> i64 {
    match v {
        Value::Int(n) => *n,
        other => panic!("expected count to be Int, got {other:?}"),
    }
}

proptest! {
    #![proptest_config(Config { cases: 64, .. Config::default() })]

    #[test]
    fn node_and_edge_counts_match_seeded((nodes, edges) in arb_graph()) {
        let store = InMemoryGraphStore::default();
        store.create_graph("g").unwrap();

        let mut node_ids = Vec::with_capacity(nodes.len());
        for spec in &nodes {
            let labels = spec.label.clone().map(|l| vec![l]).unwrap_or_default();
            let mut props: IndexMap<String, Value> = IndexMap::new();
            if let Some((k, v)) = &spec.prop {
                props.insert(k.clone(), Value::Int(*v));
            }
            node_ids.push(store.create_node("g", labels, props).unwrap());
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

        let engine = CypherEngine::new();
        let nrows = engine
            .execute(&store, "g", "MATCH (n) RETURN count(n) AS c")
            .unwrap();
        let erows = engine
            .execute(&store, "g", "MATCH ()-[r]->() RETURN count(r) AS c")
            .unwrap();

        prop_assert_eq!(count_value(&nrows.rows[0]["c"]), nodes.len() as i64);
        prop_assert_eq!(count_value(&erows.rows[0]["c"]), edges.len() as i64);
    }
}
