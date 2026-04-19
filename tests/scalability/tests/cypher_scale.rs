//! Scalability scenarios for `sedan-cypher-embedded`.
//!
//! Each scenario asserts both correctness (exact count) and a soft
//! wall-clock budget. Marked `#[ignore]` so the default `cargo test`
//! stays fast; run with:
//!
//! ```text
//! cargo test -p scalability-tests --release -- --ignored --nocapture
//! ```

use scalability_tests::{
    extract_count, fresh_store, seed_dense, seed_linear, seed_props, timed, LINEAR_EXPECTED_PATHS,
    PROP_EXPECTED_HITS,
};
use sedan_cypher_embedded::CypherEngine;

#[test]
#[ignore = "large-data scalability scenario; run with --ignored"]
fn cypher_linear_chain_100k_nodes_5_hop_paths() {
    let store = timed("cypher.linear.seed", || {
        let s = fresh_store();
        seed_linear(&s);
        s
    });
    let engine = CypherEngine::new();
    let qr = timed("cypher.linear.query", || {
        engine
            .execute(
                &store,
                "g",
                "MATCH (a:Node)-[:NEXT]->(b)-[:NEXT]->(c)-[:NEXT]->(d)-[:NEXT]->(e)-[:NEXT]->(f) \
                 RETURN count(*) AS n",
            )
            .expect("query")
    });
    let n = extract_count(&qr.columns, &qr.rows);
    assert_eq!(
        n, LINEAR_EXPECTED_PATHS as i64,
        "5-hop path count mismatch on linear chain"
    );
}

#[test]
#[ignore = "large-data scalability scenario; run with --ignored"]
fn cypher_dense_graph_10k_nodes_500k_edges_two_hop() {
    let (store, expected) = timed("cypher.dense.seed", || {
        let s = fresh_store();
        let e = seed_dense(&s);
        (s, e)
    });
    let engine = CypherEngine::new();

    // Sanity: total edge count should match what we loaded.
    let qr_edges = timed("cypher.dense.edge_count", || {
        engine
            .execute(&store, "g", "MATCH ()-[r]->() RETURN count(r) AS n")
            .expect("edge count")
    });
    assert_eq!(
        extract_count(&qr_edges.columns, &qr_edges.rows),
        scalability_tests::DENSE_EDGES as i64,
    );

    let qr = timed("cypher.dense.two_hop", || {
        engine
            .execute(
                &store,
                "g",
                "MATCH (s:Seed)-->()-->(d) RETURN count(DISTINCT d) AS n",
            )
            .expect("two-hop")
    });
    let n = extract_count(&qr.columns, &qr.rows);
    assert_eq!(
        n, expected as i64,
        "distinct two-hop neighborhood from :Seed mismatch"
    );
}

#[test]
#[ignore = "large-data scalability scenario; run with --ignored"]
fn cypher_property_scan_100k_nodes_predicate_filter() {
    let store = timed("cypher.props.seed", || {
        let s = fresh_store();
        seed_props(&s);
        s
    });
    let engine = CypherEngine::new();
    let qr = timed("cypher.props.query", || {
        engine
            .execute(
                &store,
                "g",
                "MATCH (n:Node) WHERE n.weight > 50000 AND n.parity = true RETURN count(n) AS n",
            )
            .expect("query")
    });
    let n = extract_count(&qr.columns, &qr.rows);
    assert_eq!(
        n, PROP_EXPECTED_HITS as i64,
        "property predicate count mismatch"
    );
}
