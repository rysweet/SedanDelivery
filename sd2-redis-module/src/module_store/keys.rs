// Key schema (duplicated from sd2-redis-store/src/keys.rs).
//
// These are trivial one-liners. We duplicate them here to avoid pulling the
// redis-module crate into sd2-redis-store, which the project architecture
// explicitly forbids. The key formats MUST match RedisGraphStore exactly.

use cypher_core::storage::{AdjEntry, EdgeId, NodeId};
use cypher_core::types::Value;

pub(super) const KEY_GRAPHS: &str = "__sd2_registry:graphs";

pub(super) fn key_node(g: &str, id: u64) -> String {
    format!("G:{{{g}}}:node:{id}")
}

pub(super) fn key_edge(g: &str, id: u64) -> String {
    format!("G:{{{g}}}:edge:{id}")
}

pub(super) fn key_node_seq(g: &str) -> String {
    format!("G:{{{g}}}:node_seq")
}

pub(super) fn key_edge_seq(g: &str) -> String {
    format!("G:{{{g}}}:edge_seq")
}

pub(super) fn key_label(g: &str, label: &str) -> String {
    format!("G:{{{g}}}:label:{label}")
}

pub(super) fn key_reltype(g: &str, rel_type: &str) -> String {
    format!("G:{{{g}}}:reltype:{rel_type}")
}

pub(super) fn key_out(g: &str, node_id: u64) -> String {
    format!("G:{{{g}}}:out:{node_id}")
}

pub(super) fn key_in(g: &str, node_id: u64) -> String {
    format!("G:{{{g}}}:in:{node_id}")
}

pub(super) fn key_nodes_set(g: &str) -> String {
    format!("G:{{{g}}}:nodes")
}

pub(super) fn key_edges_set(g: &str) -> String {
    format!("G:{{{g}}}:edges")
}

pub(super) fn key_indexes(g: &str) -> String {
    format!("G:{{{g}}}:indexes")
}

// Duplicated from sd2-redis-store/src/keys.rs — sd2-redis-module cannot depend
// on sd2-redis-store without pulling in the TCP client.  Keep in sync manually.
pub(super) fn key_propidx(g: &str, label: &str, property: &str, value_key: &str) -> String {
    format!("G:{{{g}}}:propidx:{label}\x00{property}\x00{value_key}")
}

pub(super) fn value_to_index_key(val: &Value) -> String {
    match val {
        Value::Int(i) => format!("i:{i}"),
        Value::Float(f) => format!("f:{}", f.to_bits()),
        Value::Str(s) => format!("s:{s}"),
        Value::Bool(b) => format!("b:{b}"),
        other => format!("j:{other:?}"),
    }
}

pub(super) fn key_graph_deleting(g: &str) -> String {
    format!("G:{{{g}}}:_deleting")
}

pub(super) fn key_label_names(g: &str) -> String {
    format!("G:{{{g}}}:_labels")
}

pub(super) fn key_reltype_names(g: &str) -> String {
    format!("G:{{{g}}}:_reltypes")
}

/// Encode an adjacency set member: "{edge_id}:{neighbor_id}:{rel_type}"
pub(super) fn encode_adj(edge_id: EdgeId, neighbor_id: NodeId, rel_type: &str) -> String {
    format!("{}:{}:{}", edge_id.0, neighbor_id.0, rel_type)
}

/// Decode an adjacency set member. Returns `None` on malformed input.
pub(super) fn decode_adj(s: &str) -> Option<AdjEntry> {
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
