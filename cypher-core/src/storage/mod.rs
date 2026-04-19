pub mod cached;
pub mod graph_name;
pub mod mem;
pub mod persistent;
pub mod types;

pub use cached::CachedGraphStore;
pub use graph_name::{validate_graph_name, GRAPH_NAME_MAX_LEN};
pub use mem::InMemoryGraphStore;
pub use persistent::{PersistentConfig, PersistentGraphStore, SyncMode};
pub use types::{
    AdjEntry, ConstraintDescriptor, ConstraintType, EdgeData, EdgeId, GraphStats, IndexDescriptor,
    NodeData, NodeId, RangeQuery, ReadConsistency,
};

use indexmap::IndexMap;
use std::any::Any;
use std::sync::Arc;

use crate::error::StorageError;
use crate::types::Value;

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
    /// Used by the IndexScan operator. Returns `StorageError::NoPropertyIndex`
    /// when no index exists — callers must handle this explicitly.
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

    /// Batch outgoing adjacency lookup aligned to `node_ids`.
    ///
    /// Default implementation preserves correctness by looping, but backends
    /// should override this to pipeline or otherwise batch remote reads.
    fn out_edges_by_type_batch(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        node_ids
            .iter()
            .map(|node_id| self.out_edges_by_type(graph, *node_id, rel_types))
            .collect()
    }

    /// Batch incoming adjacency lookup aligned to `node_ids`.
    fn in_edges_by_type_batch(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        node_ids
            .iter()
            .map(|node_id| self.in_edges_by_type(graph, *node_id, rel_types))
            .collect()
    }

    /// IDs of all outgoing edges (without neighbor data). Used by DETACH DELETE.
    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError>;

    /// IDs of all incoming edges (without neighbor data). Used by DETACH DELETE.
    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError>;

    /// All node IDs in the graph. Used for `MATCH (n) RETURN n` with no label filter.
    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError>;

    /// All edge IDs in the graph. Used for `MATCH ()-[r]->() RETURN r` with no type filter.
    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError>;

    // ── Write operations ──────────────────────────────────────────────────────

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError>;

    /// Batch-create multiple nodes and return the allocated IDs in input order.
    ///
    /// Backends with efficient batched write paths should override this. The
    /// default implementation preserves correctness by delegating to
    /// [`GraphStore::create_node`] one node at a time.
    fn batch_create_nodes(
        &self,
        graph: &str,
        nodes: Vec<(Vec<String>, IndexMap<String, Value>)>,
    ) -> Result<Vec<NodeId>, StorageError> {
        let mut created = Vec::with_capacity(nodes.len());
        for (labels, properties) in nodes {
            created.push(self.create_node(graph, labels, properties)?);
        }
        Ok(created)
    }

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, StorageError>;

    /// Batch-create multiple edges and return the allocated IDs in input order.
    ///
    /// Backends with efficient batched write paths should override this. The
    /// default implementation preserves correctness by delegating to
    /// [`GraphStore::create_edge`] one edge at a time. Backends that need to
    /// respect lower-level write limits should internally split oversized
    /// requests while preserving the overall input order and per-edge
    /// semantics.
    fn batch_create_edges(
        &self,
        graph: &str,
        edges: Vec<(NodeId, NodeId, String, IndexMap<String, Value>)>,
    ) -> Result<Vec<EdgeId>, StorageError> {
        let mut created = Vec::with_capacity(edges.len());
        for (src, dst, rel_type, properties) in edges {
            created.push(self.create_edge(graph, src, dst, rel_type, properties)?);
        }
        Ok(created)
    }

    /// Merge-update node properties: overlays given keys onto existing properties.
    /// Implements Cypher `SET n += {…}` semantics.
    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError>;

    /// Full-replace node properties: removes ALL existing properties, sets exactly the given ones.
    /// Implements Cypher `SET n = {…}` semantics.
    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError>;

    /// Merge-update edge properties. Implements Cypher `SET r += {…}` semantics.
    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError>;

    /// Full-replace edge properties. Implements Cypher `SET r = {…}` semantics.
    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
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

    // ── Constraint management ────────────────────────────────────────────────

    /// Returns all constraints defined for this graph.
    fn list_constraints(&self, _graph: &str) -> Result<Vec<ConstraintDescriptor>, StorageError> {
        Ok(vec![])
    }

    /// Create a constraint on nodes with the given label and property.
    /// Validates that existing data does not violate the constraint.
    fn create_constraint(
        &self,
        _graph: &str,
        _label: &str,
        _property: &str,
        _constraint_type: ConstraintType,
    ) -> Result<(), StorageError> {
        Err(StorageError::Backend {
            msg: "constraints not supported by this storage backend".to_string(),
        })
    }

    /// Drop a constraint identified by label and property.
    fn drop_constraint(
        &self,
        _graph: &str,
        _label: &str,
        _property: &str,
    ) -> Result<(), StorageError> {
        Err(StorageError::Backend {
            msg: "constraints not supported by this storage backend".to_string(),
        })
    }

    // ── Graph administration ──────────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError>;

    fn delete_graph(&self, name: &str) -> Result<(), StorageError>;

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError>;

    fn list_graphs(&self) -> Result<Vec<String>, StorageError>;

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError>;

    /// Returns the names of all label types currently in the graph.
    /// Used to compute the diff of labels added by a query.
    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError>;

    /// Return an optional session-scoped view of this store with the requested
    /// read-routing contract applied.
    ///
    /// Backends that do not distinguish between read topologies can ignore this
    /// contract and return `None`; callers should then continue using the
    /// original store.
    fn with_read_consistency(&self, _consistency: ReadConsistency) -> Option<Arc<dyn GraphStore>> {
        None
    }

    // ── Transaction support ──────────────────────────────────────────────────

    /// Downcast to `Any` for type-safe runtime downcasting.
    ///
    /// Required by `tx_commit` on `InMemoryGraphStore` to downcast the
    /// snapshot back to its concrete type. All implementors should return `self`.
    fn as_any(&self) -> &dyn Any;

    /// Create a copy-on-write snapshot for explicit transaction isolation.
    ///
    /// Returns `Some(snapshot)` for backends that support client-side
    /// snapshots (e.g. `InMemoryGraphStore`). Returns `None` for backends
    /// where writes are immediately durable (e.g. `RedisGraphStore`) and
    /// transactions are handled at the protocol level.
    ///
    /// The `sedandelivery-bolt` transaction handler (BEGIN/COMMIT/ROLLBACK)
    /// uses this to provide snapshot isolation when available.
    fn tx_snapshot(&self, _graphs: &[&str]) -> Result<Option<Box<dyn GraphStore>>, StorageError> {
        Ok(None)
    }

    /// Merge a transaction snapshot back into this store on COMMIT.
    ///
    /// Called by the Bolt COMMIT handler to apply mutations that were
    /// accumulated in a snapshot created by `tx_snapshot`. No-op for backends
    /// that return `None` from `tx_snapshot`.
    fn tx_commit(&self, _snapshot: &dyn GraphStore) -> Result<(), StorageError> {
        Ok(())
    }
}
