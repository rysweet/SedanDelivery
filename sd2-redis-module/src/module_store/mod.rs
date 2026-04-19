// module_store — GraphStore implementation that uses the Valkey Module API
// (ctx.call()) to execute Redis commands internally, avoiding the TCP loopback
// deadlock that prevents RedisGraphStore from working inside a module.
//
// This store uses the EXACT same key schema as RedisGraphStore so that the Bolt
// server, REST API, and GQL module all see the same graph data.
//
// Thread-safety: Valkey executes module commands on a single thread per client.
// The module context pointer is valid for the duration of one command handler.
// We stash it in a thread-local before each command and clear it afterward.
//
// The implementation is split across topical submodules:
//   * `keys`        — Redis key encoding and adjacency member encode/decode
//   * `common`      — context plumbing, ctx.call() helpers, reply parsers
//   * `nodes`       — node CRUD and lookup
//   * `edges`       — edge CRUD and lookup
//   * `adjacency`   — out/in edge enumeration
//   * `queries`     — index list/create/drop
//   * `graph_admin` — graph create/delete/list/stats
//   * `range`       — test-only range matching helpers
//
// This `mod.rs` keeps the shared struct, thread-local context, and the single
// `impl GraphStore` block (Rust requires trait impls to live in one block);
// trait methods are thin wrappers that delegate to inherent `impl_*` helpers
// defined in the topical submodules above.

use std::any::Any;
use std::cell::RefCell;

use indexmap::IndexMap;
use redis_module::Context;

use cypher_core::error::StorageError;
use cypher_core::storage::{
    AdjEntry, ConstraintDescriptor, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor,
    NodeData, NodeId, RangeQuery,
};
use cypher_core::types::Value;

mod adjacency;
mod common;
mod edges;
mod graph_admin;
mod keys;
mod nodes;
mod queries;
#[cfg(test)]
mod range;
#[cfg(test)]
mod tests;

// ── Thread-local context stash ──────────────────────────────────────────────

thread_local! {
    static MODULE_CTX: RefCell<Option<*const Context>> = const { RefCell::new(None) };
}

/// A `GraphStore` implementation backed by the Valkey Module API.
///
/// Instead of making TCP calls to Redis (which would deadlock inside a module),
/// this store calls Redis commands via `ctx.call()` — the internal dispatch path
/// that the Valkey module API provides.
///
/// The module context must be set before any GraphStore method is called, and
/// cleared afterward. This is done by the command handlers in lib.rs.
pub struct ModuleRedisStore {
    key_prefix: String,
}

impl ModuleRedisStore {
    pub fn new() -> Self {
        Self {
            key_prefix: String::new(),
        }
    }

    /// Set the module context for the current command invocation.
    /// Must be called at the start of every command handler.
    pub fn set_context(ctx: &Context) {
        MODULE_CTX.with(|cell| {
            *cell.borrow_mut() = Some(ctx as *const _);
        });
    }

    /// Clear the module context. Must be called at the end of every command handler.
    pub fn clear_context() {
        MODULE_CTX.with(|cell| {
            *cell.borrow_mut() = None;
        });
    }
}

// ── GraphStore implementation (thin delegation to topical submodules) ───────

impl GraphStore for ModuleRedisStore {
    // ── Single-item reads ───────────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        self.impl_get_node(graph, id)
    }

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        self.impl_get_edge(graph, id)
    }

    // ── Batch reads ─────────────────────────────────────────────────────────
    // Fall through to default loop-based implementations.

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        ids.iter().map(|id| self.get_node(graph, *id)).collect()
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        ids.iter().map(|id| self.get_edge(graph, *id)).collect()
    }

    // ── Label/type index reads ──────────────────────────────────────────────

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        self.impl_find_nodes_by_label(graph, label)
    }

    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        self.impl_find_nodes_by_label_and_property(graph, label, property, range)
    }

    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.impl_find_edges_by_type(graph, rel_type)
    }

    // ── Adjacency reads ─────────────────────────────────────────────────────

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.impl_out_edges(graph, node_id)
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.impl_in_edges(graph, node_id)
    }

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.impl_out_edges_by_type(graph, node_id, rel_types)
    }

    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.impl_in_edges_by_type(graph, node_id, rel_types)
    }

    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.impl_out_edge_ids(graph, node_id)
    }

    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.impl_in_edge_ids(graph, node_id)
    }

    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        self.impl_all_node_ids(graph)
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.impl_all_edge_ids(graph)
    }

    // ── Write operations ────────────────────────────────────────────────────

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        self.impl_create_node(graph, labels, properties)
    }

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        self.impl_create_edge(graph, src, dst, rel_type, properties)
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.impl_set_node_properties(graph, id, properties)
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.impl_replace_node_properties(graph, id, properties)
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.impl_set_edge_properties(graph, id, properties)
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.impl_replace_edge_properties(graph, id, properties)
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        self.impl_remove_node_property(graph, id, key)
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        self.impl_remove_edge_property(graph, id, key)
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.impl_add_node_labels(graph, id, labels)
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.impl_remove_node_labels(graph, id, labels)
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        self.impl_delete_node(graph, id, detach)
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.impl_delete_edge(graph, id)
    }

    // ── Index management ────────────────────────────────────────────────────

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        self.impl_list_indexes(graph)
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.impl_create_index(graph, label, property)
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.impl_drop_index(graph, label, property)
    }

    // ── Constraint management — not supported ───────────────────────────────

    fn list_constraints(&self, _graph: &str) -> Result<Vec<ConstraintDescriptor>, StorageError> {
        Ok(vec![])
    }

    // ── Graph administration ────────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        self.impl_create_graph(name)
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        self.impl_delete_graph(name)
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        self.impl_graph_exists(name)
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        self.impl_list_graphs()
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        self.impl_graph_stats(graph)
    }

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        self.impl_all_label_names(graph)
    }

    // ── Transaction support — not supported ─────────────────────────────────

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn tx_snapshot(&self, _graphs: &[&str]) -> Result<Option<Box<dyn GraphStore>>, StorageError> {
        Ok(None)
    }

    fn tx_commit(&self, _snapshot: &dyn GraphStore) -> Result<(), StorageError> {
        Ok(())
    }
}

// ModuleRedisStore contains only owned, thread-safe data (a String for the
// key prefix); the auto-derived Send + Sync are sufficient. The Valkey
// `Context` is not stored on the struct — it lives in a per-thread
// `MODULE_CTX` and is only valid for the duration of a command callback.
