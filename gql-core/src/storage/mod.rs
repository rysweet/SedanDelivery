pub mod mem;

pub use mem::InMemoryGraphStore;

use std::collections::HashMap;

use crate::error::StorageError;
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
    pub properties: HashMap<String, Value>,
}

/// The data for a single edge as returned from storage.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeData {
    pub id: EdgeId,
    pub rel_type: String,
    pub src: NodeId,
    pub dst: NodeId,
    pub properties: HashMap<String, Value>,
}

/// An adjacency list entry returned by `out_edges` / `in_edges`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdjEntry {
    pub edge_id: EdgeId,
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

// ── The core abstraction boundary ─────────────────────────────────────────────

/// The trait that all graph storage backends must implement.
///
/// Design invariants:
/// - All methods are synchronous. The Redis module runs in Redis's main thread.
///   Background threads use `RedisModule_BlockClient`; this trait does not model that.
/// - Graph names are validated before calling trait methods. Implementations may
///   assume the graph name is already sanitized (no `:`, no `{}`).
/// - Batch methods (`get_nodes_batch`, `get_edges_batch`) are critical for meeting
///   latency targets. Do not skip them. See architecture review note in PLAN.md §5.
pub trait GraphStore: Send + Sync {
    // ── Single-item reads ─────────────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError>;

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError>;

    // ── Batch reads (CRITICAL for traversal performance) ─────────────────────

    /// Fetch multiple nodes in one operation. Implementations MUST batch
    /// (pipeline Redis commands or read from an in-memory map in one pass).
    /// Returns one entry per input ID; missing nodes are `None`.
    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError>;

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError>;

    // ── Label/type index reads ────────────────────────────────────────────────

    /// All node IDs with the given label. Used for full label scans (NodeScan operator).
    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError>;

    /// Nodes with a given label AND a property satisfying a range predicate.
    /// Used by the IndexScan operator. Falls back to NodeScan + Filter if no index exists.
    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError>;

    /// All edge IDs of the given relationship type.
    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError>;

    // ── Adjacency reads ───────────────────────────────────────────────────────

    /// All outgoing edges from a node, in any order.
    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError>;

    /// All incoming edges to a node, in any order.
    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError>;

    /// Outgoing edges filtered to the given relationship types (empty = all types).
    /// MUST be more efficient than `out_edges` + client-side filter when rel_types is non-empty.
    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError>;

    /// Incoming edges filtered to the given relationship types.
    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError>;

    /// IDs of all outgoing edges (without neighbor data). Used by DETACH DELETE.
    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError>;

    /// IDs of all incoming edges (without neighbor data). Used by DETACH DELETE.
    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError>;

    /// All node IDs in the graph. Used for `MATCH (n) RETURN n` with no label filter.
    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError>;

    // ── Write operations ──────────────────────────────────────────────────────

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: HashMap<String, Value>,
    ) -> Result<NodeId, StorageError>;

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: HashMap<String, Value>,
    ) -> Result<EdgeId, StorageError>;

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError>;

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError>;

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError>;

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError>;

    /// Add labels to a node (MERGE ON CREATE SET n:Label).
    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError>;

    /// Remove labels from a node (REMOVE n:Label).
    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError>;

    /// Delete a node. If `detach` is false and the node has relationships,
    /// returns `Err` (Cypher semantics: must use DETACH DELETE).
    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError>;

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError>;

    // ── Index management ──────────────────────────────────────────────────────

    /// Returns all (label, property) indexes defined for this graph.
    /// The query planner uses this to decide between NodeScan and IndexScan.
    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError>;

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError>;

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError>;

    // ── Graph administration ──────────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError>;

    fn delete_graph(&self, name: &str) -> Result<(), StorageError>;

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError>;

    fn list_graphs(&self) -> Result<Vec<String>, StorageError>;

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError>;
}

/// Validate a graph name: must be non-empty, must not contain `:` (Redis key separator)
/// or `{`/`}` (Redis hash tags, which would interfere with cluster slot routing).
pub fn validate_graph_name(name: &str) -> Result<(), StorageError> {
    if name.is_empty() {
        return Err(StorageError::InvalidGraphName {
            name: name.to_string(),
            reason: "graph name must not be empty",
        });
    }
    if name.contains(':') {
        return Err(StorageError::InvalidGraphName {
            name: name.to_string(),
            reason: "graph name must not contain ':' (reserved as Redis key separator)",
        });
    }
    if name.contains('{') || name.contains('}') {
        return Err(StorageError::InvalidGraphName {
            name: name.to_string(),
            reason: "graph name must not contain '{' or '}' (reserved as Redis hash tags)",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_graph_names() {
        assert!(validate_graph_name("social").is_ok());
        assert!(validate_graph_name("my-graph").is_ok());
        assert!(validate_graph_name("graph_1").is_ok());
    }

    #[test]
    fn invalid_graph_names() {
        // Empty
        assert!(validate_graph_name("").is_err());
        // Contains colon
        assert!(validate_graph_name("foo:bar").is_err());
        // Contains Redis hash tag
        assert!(validate_graph_name("{myslot}graph").is_err());
        assert!(validate_graph_name("graph}end").is_err());
    }
}
