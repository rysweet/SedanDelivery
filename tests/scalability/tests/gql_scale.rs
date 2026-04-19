//! Scalability scenarios for `sedan-gql-embedded` (GQL/Cypher engine).
//!
//! See `cypher_scale.rs` for the equivalent scenarios on the openCypher
//! surface. Run with:
//!
//! ```text
//! cargo test -p scalability-tests --release -- --ignored --nocapture
//! ```

use scalability_tests::{
    extract_count, fresh_store, seed_dense, seed_linear, seed_props, timed, DENSE_EDGES,
    LINEAR_EXPECTED_PATHS, PROP_EXPECTED_HITS,
};
use sedan_gql_embedded::SedanGql;

#[test]
#[ignore = "large-data scalability scenario; run with --ignored"]
fn gql_linear_chain_100k_nodes_5_hop_paths() {
    let store = timed("gql.linear.seed", || {
        let s = fresh_store();
        seed_linear(&s);
        s
    });
    let engine = SedanGql::new();
    let qr = timed("gql.linear.query", || {
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
    assert_eq!(n, LINEAR_EXPECTED_PATHS as i64);
}

#[test]
#[ignore = "large-data scalability scenario; run with --ignored"]
fn gql_dense_graph_10k_nodes_500k_edges_two_hop() {
    let (store, expected) = timed("gql.dense.seed", || {
        let s = fresh_store();
        let e = seed_dense(&s);
        (s, e)
    });
    let engine = SedanGql::new();

    let qr_edges = timed("gql.dense.edge_count", || {
        engine
            .execute(&store, "g", "MATCH ()-[r]->() RETURN count(r) AS n")
            .expect("edge count")
    });
    assert_eq!(
        extract_count(&qr_edges.columns, &qr_edges.rows),
        DENSE_EDGES as i64,
    );

    let qr = timed("gql.dense.two_hop", || {
        engine
            .execute(
                &store,
                "g",
                "MATCH (s:Seed)-->()-->(d) RETURN count(DISTINCT d) AS n",
            )
            .expect("two-hop")
    });
    let n = extract_count(&qr.columns, &qr.rows);
    assert_eq!(n, expected as i64);
}

#[test]
#[ignore = "large-data scalability scenario; run with --ignored"]
fn gql_property_scan_100k_nodes_predicate_filter() {
    let store = timed("gql.props.seed", || {
        let s = fresh_store();
        seed_props(&s);
        s
    });
    let engine = SedanGql::new();
    let qr = timed("gql.props.query", || {
        engine
            .execute(
                &store,
                "g",
                "MATCH (n:Node) WHERE n.weight > 50000 AND n.parity = true RETURN count(n) AS n",
            )
            .expect("query")
    });
    let n = extract_count(&qr.columns, &qr.rows);
    assert_eq!(n, PROP_EXPECTED_HITS as i64);
}
