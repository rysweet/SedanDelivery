// keys.rs — Redis key formatting and adjacency encoding helpers.

use gql_core::storage::{AdjEntry, EdgeId, NodeId};
use gql_core::types::Value;

// ── Adjacency encoding ──────────────────────────────────────────────────────

/// Encode an adjacency set member: "{edge_id}:{neighbor_id}:{rel_type}"
pub(crate) fn encode_adj(edge_id: EdgeId, neighbor_id: NodeId, rel_type: &str) -> String {
    format!("{}:{}:{}", edge_id.0, neighbor_id.0, rel_type)
}

/// Decode an adjacency set member. Returns `None` on malformed input.
pub(crate) fn decode_adj(s: &str) -> Option<AdjEntry> {
    // Format: "{edge_id}:{neighbor_id}:{rel_type}"
    // rel_type may contain colons, so split only on the first two colons.
    let mut parts = s.splitn(3, ':');
    let edge_raw = parts.next()?;
    let neighbor_raw = parts.next()?;
    let rel_type = parts.next()?.to_string();
    let edge_id = EdgeId(edge_raw.parse().ok()?);
    let neighbor_id = NodeId(neighbor_raw.parse().ok()?);
    Some(AdjEntry {
        edge_id,
        neighbor_id,
        rel_type,
    })
}

// ── Key helpers ─────────────────────────────────────────────────────────────

pub(crate) fn key_node(g: &str, id: u64) -> String {
    format!("G:{{{g}}}:node:{id}")
}

pub(crate) fn key_edge(g: &str, id: u64) -> String {
    format!("G:{{{g}}}:edge:{id}")
}

pub(crate) fn key_node_seq(g: &str) -> String {
    format!("G:{{{g}}}:node_seq")
}

pub(crate) fn key_edge_seq(g: &str) -> String {
    format!("G:{{{g}}}:edge_seq")
}

pub(crate) fn key_label(g: &str, label: &str) -> String {
    format!("G:{{{g}}}:label:{label}")
}

pub(crate) fn key_reltype(g: &str, rel_type: &str) -> String {
    format!("G:{{{g}}}:reltype:{rel_type}")
}

pub(crate) fn key_out(g: &str, node_id: u64) -> String {
    format!("G:{{{g}}}:out:{node_id}")
}

pub(crate) fn key_in(g: &str, node_id: u64) -> String {
    format!("G:{{{g}}}:in:{node_id}")
}

pub(crate) fn key_nodes_set(g: &str) -> String {
    format!("G:{{{g}}}:nodes")
}

pub(crate) fn key_edges_set(g: &str) -> String {
    format!("G:{{{g}}}:edges")
}

pub(crate) fn key_indexes(g: &str) -> String {
    format!("G:{{{g}}}:indexes")
}

/// Temporary marker set while `delete_graph` cleanup is in progress.
/// Public reads treat a graph with this marker as already deleted, but the
/// marker keeps the name reserved so cleanup can be retried after partial
/// failures.
pub(crate) fn key_graph_deleting(g: &str) -> String {
    format!("G:{{{g}}}:_deleting")
}

/// Registry of label names seen in a graph. Used for cluster-safe label
/// enumeration without SCAN.
pub(crate) fn key_label_names(g: &str) -> String {
    format!("G:{{{g}}}:_labels")
}

/// Registry of relationship type names seen in a graph. Used for cluster-safe
/// reltype enumeration without SCAN.
pub(crate) fn key_reltype_names(g: &str) -> String {
    format!("G:{{{g}}}:_reltypes")
}

/// Separator for label/property in index metadata and propidx keys.
/// Using a null byte avoids collisions when label or property names
/// themselves contain colons.
pub(crate) const IDX_SEP: char = '\x00';

/// Property index key: maps (label, property, value) -> SET of node IDs.
/// Enables O(1) MERGE lookups instead of O(N) full-label scans.
pub(crate) fn key_propidx(g: &str, label: &str, property: &str, value_key: &str) -> String {
    format!(
        "G:{{{g}}}:propidx:{label}{}{property}{}{value_key}",
        IDX_SEP, IDX_SEP
    )
}

/// Manifest of propidx keys created for a graph.
///
/// Index backfill and indexed writes are intentionally non-atomic, so
/// `delete_graph` needs a durable way to find propidx sets even if index
/// metadata registration fails part-way through.
pub(crate) fn key_propidx_manifest(g: &str) -> String {
    format!("G:{{{g}}}:_propidx")
}

/// Convert a Value to a canonical string for use as a Redis key component.
/// Must be deterministic: same value always produces same string.
pub(crate) fn value_to_index_key(val: &Value) -> String {
    match val {
        Value::Int(i) => format!("i:{i}"),
        Value::Float(f) => format!("f:{}", f.to_bits()), // bitwise for determinism
        Value::Str(s) => format!("s:{s}"),
        Value::Bool(b) => format!("b:{b}"),
        // For complex types, use Debug representation.
        other => format!("j:{other:?}"),
    }
}

/// Distributed write-lock key for a graph.  Used with CAS-release Lua to
/// coordinate concurrent write operations on the same graph.
pub(crate) fn key_graph_lock(g: &str) -> String {
    format!("G:{{{g}}}:write_lock")
}

/// Registry key for graph names.  Uses a `__sd2_registry:` prefix that cannot
/// collide with per-graph data keys (which all start with `G:`).  The key has
/// no Redis Cluster hash tag, so it routes to its own slot; this is fine because
/// it's only accessed by standalone operations (SADD, SISMEMBER, SMEMBERS, SREM).
pub(crate) const KEY_GRAPHS: &str = "__sd2_registry:graphs";

#[cfg(test)]
mod tests {
    use super::*;

    // ── encode_adj / decode_adj round-trip ───────────────────────────────────

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
    fn encode_adj_with_zero_ids() {
        let encoded = encode_adj(EdgeId(0), NodeId(0), "REL");
        assert_eq!(encoded, "0:0:REL");
        let decoded = decode_adj(&encoded).unwrap();
        assert_eq!(decoded.edge_id, EdgeId(0));
        assert_eq!(decoded.neighbor_id, NodeId(0));
        assert_eq!(decoded.rel_type, "REL");
    }

    #[test]
    fn encode_adj_with_large_ids() {
        let encoded = encode_adj(EdgeId(u64::MAX), NodeId(u64::MAX), "TYPE");
        let decoded = decode_adj(&encoded).unwrap();
        assert_eq!(decoded.edge_id, EdgeId(u64::MAX));
        assert_eq!(decoded.neighbor_id, NodeId(u64::MAX));
        assert_eq!(decoded.rel_type, "TYPE");
    }

    #[test]
    fn decode_adj_rel_type_with_colons() {
        // rel_type may contain colons; only the first two colons are split on.
        let encoded = "5:10:HAS:COLON:IN:NAME";
        let decoded = decode_adj(encoded).unwrap();
        assert_eq!(decoded.edge_id, EdgeId(5));
        assert_eq!(decoded.neighbor_id, NodeId(10));
        assert_eq!(decoded.rel_type, "HAS:COLON:IN:NAME");
    }

    #[test]
    fn decode_adj_malformed_input_returns_none() {
        assert!(decode_adj("").is_none());
        assert!(decode_adj("single").is_none());
        assert!(decode_adj("one:two").is_none()); // only 2 parts, no rel_type
        assert!(decode_adj("notanumber:20:REL").is_none());
        assert!(decode_adj("10:notanumber:REL").is_none());
    }

    // ── Key generation helpers ──────────────────────────────────────────────

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
    fn key_propidx_format() {
        assert_eq!(
            key_propidx("db", "Person", "name", "s:Alice"),
            "G:{db}:propidx:Person\x00name\x00s:Alice"
        );
    }

    #[test]
    fn key_propidx_manifest_format() {
        assert_eq!(key_propidx_manifest("db"), "G:{db}:_propidx");
    }

    #[test]
    fn key_graphs_constant() {
        assert_eq!(KEY_GRAPHS, "__sd2_registry:graphs");
        // Must NOT start with "G:" to avoid collisions with per-graph data.
        assert!(!KEY_GRAPHS.starts_with("G:"));
    }

    // ── All keys use the graph hash tag {g} for Redis Cluster ───────────────

    #[test]
    fn all_graph_keys_contain_hash_tag() {
        let g = "testgraph";
        let tag = format!("{{{g}}}");
        // Every per-graph key must include the hash tag for slot affinity.
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
        assert!(key_graph_lock(g).contains(&tag));
        assert!(key_graph_deleting(g).contains(&tag));
        assert!(key_label_names(g).contains(&tag));
        assert!(key_reltype_names(g).contains(&tag));
        assert!(key_propidx(g, "L", "p", "v").contains(&tag));
        assert!(key_propidx_manifest(g).contains(&tag));
    }

    // ── value_to_index_key ──────────────────────────────────────────────────

    #[test]
    fn value_to_index_key_int() {
        assert_eq!(value_to_index_key(&Value::Int(42)), "i:42");
        assert_eq!(value_to_index_key(&Value::Int(-1)), "i:-1");
        assert_eq!(value_to_index_key(&Value::Int(0)), "i:0");
    }

    #[test]
    fn value_to_index_key_float() {
        let k = value_to_index_key(&Value::Float(3.14));
        assert!(k.starts_with("f:"));
        // Determinism: same float produces same key.
        assert_eq!(k, value_to_index_key(&Value::Float(3.14)));
    }

    #[test]
    fn value_to_index_key_string() {
        assert_eq!(value_to_index_key(&Value::Str("hello".into())), "s:hello");
        assert_eq!(value_to_index_key(&Value::Str("".into())), "s:");
    }

    #[test]
    fn value_to_index_key_bool() {
        assert_eq!(value_to_index_key(&Value::Bool(true)), "b:true");
        assert_eq!(value_to_index_key(&Value::Bool(false)), "b:false");
    }

    #[test]
    fn value_to_index_key_null_uses_debug() {
        let k = value_to_index_key(&Value::Null);
        assert!(k.starts_with("j:"));
    }

    #[test]
    fn value_to_index_key_deterministic_for_same_float() {
        // NaN bits: ensure bitwise representation is used.
        let a = value_to_index_key(&Value::Float(f64::NAN));
        let b = value_to_index_key(&Value::Float(f64::NAN));
        assert_eq!(a, b, "NaN must produce the same key every time");
    }
}
