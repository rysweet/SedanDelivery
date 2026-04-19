#![cfg(test)]

use cypher_core::serde_value::{json_to_props, props_to_json};
use cypher_core::storage::{EdgeId, NodeId, RangeQuery};
use cypher_core::types::Value;
use indexmap::IndexMap;

use super::keys::{
    decode_adj, encode_adj, key_edge, key_edge_seq, key_edges_set, key_graph_deleting, key_in,
    key_indexes, key_label, key_label_names, key_node, key_node_seq, key_nodes_set, key_out,
    key_propidx, key_reltype, key_reltype_names, KEY_GRAPHS,
};
use super::range::range_matches;

// ── Key schema compatibility tests ──────────────────────────────────────
// Verify that the key functions here produce identical output to
// sd2-redis-store/src/keys.rs.

#[test]
fn key_node_format() {
    assert_eq!(key_node("mydb", 42), "G:{mydb}:node:42");
    assert_eq!(key_node("test", 0), "G:{test}:node:0");
}

#[test]
fn key_edge_format() {
    assert_eq!(key_edge("mydb", 99), "G:{mydb}:edge:99");
}

#[test]
fn key_node_seq_format() {
    assert_eq!(key_node_seq("g1"), "G:{g1}:node_seq");
}

#[test]
fn key_edge_seq_format() {
    assert_eq!(key_edge_seq("g1"), "G:{g1}:edge_seq");
}

#[test]
fn key_label_format() {
    assert_eq!(key_label("mydb", "Person"), "G:{mydb}:label:Person");
}

#[test]
fn key_reltype_format() {
    assert_eq!(key_reltype("mydb", "KNOWS"), "G:{mydb}:reltype:KNOWS");
}

#[test]
fn key_out_format() {
    assert_eq!(key_out("mydb", 7), "G:{mydb}:out:7");
}

#[test]
fn key_in_format() {
    assert_eq!(key_in("mydb", 7), "G:{mydb}:in:7");
}

#[test]
fn key_nodes_set_format() {
    assert_eq!(key_nodes_set("g"), "G:{g}:nodes");
}

#[test]
fn key_edges_set_format() {
    assert_eq!(key_edges_set("g"), "G:{g}:edges");
}

#[test]
fn key_indexes_format() {
    assert_eq!(key_indexes("db"), "G:{db}:indexes");
}

#[test]
fn key_graph_deleting_format() {
    assert_eq!(key_graph_deleting("db"), "G:{db}:_deleting");
}

#[test]
fn key_label_names_format() {
    assert_eq!(key_label_names("db"), "G:{db}:_labels");
}

#[test]
fn key_reltype_names_format() {
    assert_eq!(key_reltype_names("db"), "G:{db}:_reltypes");
}

#[test]
fn key_graphs_constant() {
    assert_eq!(KEY_GRAPHS, "__sd2_registry:graphs");
    assert!(!KEY_GRAPHS.starts_with("G:"));
}

#[test]
fn all_graph_keys_contain_hash_tag() {
    let g = "testgraph";
    let tag = format!("{{{g}}}");
    assert!(key_node(g, 1).contains(&tag));
    assert!(key_edge(g, 1).contains(&tag));
    assert!(key_node_seq(g).contains(&tag));
    assert!(key_edge_seq(g).contains(&tag));
    assert!(key_label(g, "L").contains(&tag));
    assert!(key_reltype(g, "R").contains(&tag));
    assert!(key_out(g, 1).contains(&tag));
    assert!(key_in(g, 1).contains(&tag));
    assert!(key_nodes_set(g).contains(&tag));
    assert!(key_edges_set(g).contains(&tag));
    assert!(key_indexes(g).contains(&tag));
    assert!(key_propidx(g, "Person", "name", "s:Alice").contains(&tag));
    assert!(key_graph_deleting(g).contains(&tag));
    assert!(key_label_names(g).contains(&tag));
    assert!(key_reltype_names(g).contains(&tag));
}

// ── Adjacency encoding ─────────────────────────────────────────────────

#[test]
fn encode_decode_adj_round_trip() {
    let encoded = encode_adj(EdgeId(10), NodeId(20), "KNOWS");
    assert_eq!(encoded, "10:20:KNOWS");
    let decoded = decode_adj(&encoded).expect("should decode");
    assert_eq!(decoded.edge_id, EdgeId(10));
    assert_eq!(decoded.neighbor_id, NodeId(20));
    assert_eq!(decoded.rel_type, "KNOWS");
}

#[test]
fn decode_adj_with_colons_in_rel_type() {
    let encoded = "5:10:HAS:COLON:IN:NAME";
    let decoded = decode_adj(encoded).unwrap();
    assert_eq!(decoded.edge_id, EdgeId(5));
    assert_eq!(decoded.neighbor_id, NodeId(10));
    assert_eq!(decoded.rel_type, "HAS:COLON:IN:NAME");
}

#[test]
fn decode_adj_malformed_returns_none() {
    assert!(decode_adj("").is_none());
    assert!(decode_adj("single").is_none());
    assert!(decode_adj("one:two").is_none());
    assert!(decode_adj("notanumber:20:REL").is_none());
}

// ── Range matching ──────────────────────────────────────────────────────

#[test]
fn range_eq_match() {
    assert!(range_matches(
        &Value::Int(42),
        &RangeQuery::Eq(&Value::Int(42))
    ));
    assert!(!range_matches(
        &Value::Int(42),
        &RangeQuery::Eq(&Value::Int(43))
    ));
}

#[test]
fn range_starts_with() {
    assert!(range_matches(
        &Value::Str("hello world".into()),
        &RangeQuery::StartsWith("hello")
    ));
    assert!(!range_matches(
        &Value::Str("goodbye".into()),
        &RangeQuery::StartsWith("hello")
    ));
}

// ── JSON serialization ──────────────────────────────────────────────────

#[test]
fn props_json_round_trip() {
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    props.insert("age".to_string(), Value::Int(30));
    let json = props_to_json(&props).unwrap();
    let parsed = json_to_props(&json).unwrap();
    assert_eq!(parsed.get("name"), Some(&Value::Str("Alice".into())));
    assert_eq!(parsed.get("age"), Some(&Value::Int(30)));
}

#[test]
fn labels_json_round_trip() {
    let labels = vec!["Person".to_string(), "Employee".to_string()];
    let json = serde_json::to_string(&labels).unwrap();
    let parsed: Vec<String> = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, labels);
}
