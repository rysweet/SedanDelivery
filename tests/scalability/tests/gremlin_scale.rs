//! Scalability scenarios for `sedan-gremlin-embedded`.
//!
//! Mirrors `cypher_scale.rs` and `gql_scale.rs`, expressed as Gremlin
//! traversals. Run with:
//!
//! ```text
//! cargo test -p scalability-tests --release -- --ignored --nocapture
//! ```

use scalability_tests::{
    extract_count, fresh_store, seed_dense, seed_linear, seed_props, timed, DENSE_EDGES,
    LINEAR_EXPECTED_PATHS, PROP_EXPECTED_HITS,
};
use sedan_gremlin_embedded::GremlinEngine;

#[test]
#[ignore = "large-data scalability scenario; run with --ignored"]
fn gremlin_linear_chain_100k_nodes_5_hop_paths() {
    let store = timed("gremlin.linear.seed", || {
        let s = fresh_store();
        seed_linear(&s);
        s
    });
    let engine = GremlinEngine::new();
    // Each starting vertex contributes its 5-hop path count; on a single
    // chain of N nodes that is exactly N - 5 paths.
    let qr = timed("gremlin.linear.query", || {
        engine
            .execute(
                &store,
                "g",
                "g.V().hasLabel('Node').out('NEXT').out('NEXT').out('NEXT').out('NEXT').out('NEXT').count()",
            )
            .expect("query")
    });
    let n = extract_count(&qr.columns, &qr.rows);
    assert_eq!(n, LINEAR_EXPECTED_PATHS as i64);
}

#[test]
#[ignore = "large-data scalability scenario; run with --ignored"]
fn gremlin_dense_graph_10k_nodes_500k_edges_two_hop() {
    let (store, expected) = timed("gremlin.dense.seed", || {
        let s = fresh_store();
        let e = seed_dense(&s);
        (s, e)
    });
    let engine = GremlinEngine::new();

    let qr_edges = timed("gremlin.dense.edge_count", || {
        engine
            .execute(&store, "g", "g.E().count()")
            .expect("edge count")
    });
    assert_eq!(
        extract_count(&qr_edges.columns, &qr_edges.rows),
        DENSE_EDGES as i64,
    );

    let qr = timed("gremlin.dense.two_hop", || {
        engine
            .execute(
                &store,
                "g",
                "g.V().hasLabel('Seed').out().out().dedup().count()",
            )
            .expect("two-hop")
    });
    let n = extract_count(&qr.columns, &qr.rows);
    assert_eq!(n, expected as i64);
}

#[test]
#[ignore = "large-data scalability scenario; run with --ignored"]
fn gremlin_property_scan_100k_nodes_predicate_filter() {
    let store = timed("gremlin.props.seed", || {
        let s = fresh_store();
        seed_props(&s);
        s
    });
    let engine = GremlinEngine::new();
    let qr = timed("gremlin.props.query", || {
        engine
            .execute(
                &store,
                "g",
                "g.V().hasLabel('Node').has('weight', gt(50000)).has('parity', true).count()",
            )
            .expect("query")
    });
    let n = extract_count(&qr.columns, &qr.rows);
    assert_eq!(n, PROP_EXPECTED_HITS as i64);
}
