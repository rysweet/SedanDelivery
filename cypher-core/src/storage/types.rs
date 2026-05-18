//! Data transfer types and newtype IDs used by the [`GraphStore`](super::GraphStore) trait.
//!
//! These types form the contract between the storage layer and its callers
//! (planner, executor, protocol layers). They are re-exported from
//! [`crate::storage`] so existing `cypher_core::storage::*` paths continue to
//! resolve.

use indexmap::IndexMap;

use crate::types::Value;

// ── Newtype IDs ───────────────────────────────────────────────────────────────

/// Strongly-typed node identifier.
///
/// SECURITY NOTE (from security review): u32 wraps silently at 4.29B with data corruption.
/// We use u64 (max 9.2×10^18). The Redis INCR counter is i64; values ≤ 0 indicate overflow
/// and must be rejected by the storage layer with `StorageError::IdOverflow`.
/// See security review notes and ADR-001.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

/// Strongly-typed edge identifier. Uses u64 for the same reasons as NodeId.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EdgeId(pub u64);

// ── Data transfer types ───────────────────────────────────────────────────────

/// The data for a single node as returned from storage.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeData {
    pub id: NodeId,
    pub labels: Vec<String>,
    pub properties: IndexMap<String, Value>,
}

/// The data for a single edge as returned from storage.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeData {
    pub id: EdgeId,
    pub rel_type: String,
    pub src: NodeId,
    pub dst: NodeId,
    pub properties: IndexMap<String, Value>,
}

/// An adjacency list entry returned by `out_edges` / `in_edges`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdjEntry {
    pub edge_id: EdgeId,
    /// The node at the other end of the edge (destination for out-edges, source for in-edges).
    pub neighbor_id: NodeId,
    pub rel_type: String,
}

/// Graph-level statistics returned by `graph_stats`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphStats {
    pub node_count: u64,
    pub edge_count: u64,
    pub label_count: u32,
    pub rel_type_count: u32,
}

/// Describes a property index for use by the query planner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDescriptor {
    pub label: String,
    pub property: String,
}

/// The type of a constraint.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ConstraintType {
    /// Property values must be unique across all nodes with the given label.
    Unique,
}

impl std::fmt::Display for ConstraintType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConstraintType::Unique => write!(f, "UNIQUE"),
        }
    }
}

/// Describes a constraint for use by the admin layer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConstraintDescriptor {
    pub label: String,
    pub property: String,
    pub constraint_type: ConstraintType,
}

/// A range predicate for property index lookups.
#[derive(Debug, Clone, PartialEq)]
pub enum RangeQuery<'a> {
    Eq(&'a Value),
    Lt(&'a Value),
    Lte(&'a Value),
    Gt(&'a Value),
    Gte(&'a Value),
    Between {
        lo: &'a Value,
        hi: &'a Value,
        inclusive: bool,
    },
    StartsWith(&'a str), // string prefix scan
}

/// Session-scoped read-routing contract passed from protocol/front-end layers
/// into storage implementations that support topology-aware reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadConsistency {
    /// Use the backend's normal topology policy (for example replica-first).
    TopologyDefault,
    /// Route reads to the primary because the session has a recent write and
    /// must observe read-your-writes behavior.
    SessionFreshPrimary,
}
