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

/// Property index key: maps (label, property, value) -> SET of node IDs.
/// Enables O(1) MERGE lookups instead of O(N) full-label scans.
pub(crate) fn key_propidx(g: &str, label: &str, property: &str, value_key: &str) -> String {
    format!("G:{{{g}}}:propidx:{label}:{property}:{value_key}")
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

/// Registry key for graph names.  Uses a `__sd2_registry:` prefix that cannot
/// collide with per-graph data keys (which all start with `G:`).  The key has
/// no Redis Cluster hash tag, so it routes to its own slot; this is fine because
/// it's only accessed by standalone operations (SADD, SISMEMBER, SMEMBERS, SREM).
pub(crate) const KEY_GRAPHS: &str = "__sd2_registry:graphs";
