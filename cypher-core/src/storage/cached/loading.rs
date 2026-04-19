//! Cache loading and invalidation.
//!
//! These methods lazily load a full graph from the backend into the
//! in-memory cache (preserving backend-allocated IDs) and provide the
//! `invalidate` helper used when a graph is dropped.

use std::sync::Arc;

use crate::error::StorageError;
use crate::storage::{AdjEntry, EdgeData, GraphStore, NodeData};

use super::CachedGraphStore;

impl CachedGraphStore {
    /// Ensure the named graph is loaded in the cache.
    /// If already loaded, this is a no-op.
    pub(super) fn ensure_loaded(&self, graph: &str) -> Result<(), StorageError> {
        // Fast path: check with read lock.
        {
            let loaded = self
                .loaded
                .read()
                .map_err(|e| StorageError::backend(format!("lock poisoned: {e}")))?;
            if loaded.contains(graph) {
                return Ok(());
            }
        }
        // Slow path: load from backend.
        self.load_graph(graph)
    }

    /// Load a full graph from the backend into the cache.
    pub(super) fn load_graph(&self, graph: &str) -> Result<(), StorageError> {
        // Check if the graph exists on the backend.
        if !self.backend.graph_exists(graph)? {
            return Err(StorageError::GraphNotFound {
                name: graph.to_string(),
            });
        }

        // If graph already exists in cache, drop it first to get a clean slate.
        if self.cache.graph_exists(graph).unwrap_or(false) {
            let _ = self.cache.delete_graph(graph);
        }
        self.cache.create_graph(graph)?;

        // Load all nodes.
        let node_ids = self.backend.all_node_ids(graph)?;
        if !node_ids.is_empty() {
            let nodes = self.backend.get_nodes_batch(graph, &node_ids)?;
            for maybe_node in nodes.into_iter().flatten() {
                // We need to inject the node with its original ID.
                // InMemoryGraphStore allocates its own IDs, so we use a
                // direct-insert approach via create_node + property injection.
                // Actually, we need to directly manipulate the inner state.
                self.inject_node_into_cache(graph, maybe_node)?;
            }
        }

        // Load all edges.
        let edge_ids = self.backend.all_edge_ids(graph)?;
        if !edge_ids.is_empty() {
            let edges = self.backend.get_edges_batch(graph, &edge_ids)?;
            for maybe_edge in edges.into_iter().flatten() {
                self.inject_edge_into_cache(graph, maybe_edge)?;
            }
        }

        // Load indexes.
        let indexes = self.backend.list_indexes(graph)?;
        for idx in indexes {
            let _ = self.cache.create_index(graph, &idx.label, &idx.property);
        }

        // Load constraints.
        let constraints = self.backend.list_constraints(graph)?;
        for c in constraints {
            let _ = self
                .cache
                .create_constraint(graph, &c.label, &c.property, c.constraint_type);
        }

        // Mark as loaded.
        let mut loaded = self
            .loaded
            .write()
            .map_err(|e| StorageError::backend(format!("lock poisoned: {e}")))?;
        loaded.insert(graph.to_string());
        Ok(())
    }

    /// Inject a node directly into the cache's internal state preserving the original ID.
    pub(super) fn inject_node_into_cache(
        &self,
        graph: &str,
        node: NodeData,
    ) -> Result<(), StorageError> {
        let mut guard = self
            .cache
            .inner
            .write()
            .map_err(|e| StorageError::backend(format!("lock poisoned: {e}")))?;
        let graphs = Arc::make_mut(&mut guard);
        let g = graphs
            .get_mut(graph)
            .ok_or_else(|| StorageError::GraphNotFound {
                name: graph.to_string(),
            })?;

        let raw_id = node.id.0;

        // Update sequence counter to be at least as high as this ID.
        if raw_id > g.edge_seq {
            g.edge_seq = raw_id;
            g.node_seq = raw_id;
        }

        // Update label index.
        for label in &node.labels {
            g.label_index
                .entry(label.clone())
                .or_default()
                .insert(raw_id);
        }

        // Update property indexes.
        g.index_insert(raw_id, &node.labels, &node.properties);

        // Initialize adjacency lists.
        g.out_adj.entry(raw_id).or_default();
        g.in_adj.entry(raw_id).or_default();

        // Insert node.
        g.nodes.insert(raw_id, node);

        Ok(())
    }

    /// Inject an edge directly into the cache's internal state preserving the original ID.
    pub(super) fn inject_edge_into_cache(
        &self,
        graph: &str,
        edge: EdgeData,
    ) -> Result<(), StorageError> {
        let mut guard = self
            .cache
            .inner
            .write()
            .map_err(|e| StorageError::backend(format!("lock poisoned: {e}")))?;
        let graphs = Arc::make_mut(&mut guard);
        let g = graphs
            .get_mut(graph)
            .ok_or_else(|| StorageError::GraphNotFound {
                name: graph.to_string(),
            })?;

        let raw_id = edge.id.0;

        // Update sequence counter.
        if raw_id > g.edge_seq {
            g.edge_seq = raw_id;
        }

        // Update rel_type index.
        g.reltype_index
            .entry(edge.rel_type.clone())
            .or_default()
            .insert(raw_id);

        // Update adjacency lists.
        let out_entry = AdjEntry {
            edge_id: edge.id,
            neighbor_id: edge.dst,
            rel_type: edge.rel_type.clone(),
        };
        g.out_adj.entry(edge.src.0).or_default().push(out_entry);

        let in_entry = AdjEntry {
            edge_id: edge.id,
            neighbor_id: edge.src,
            rel_type: edge.rel_type.clone(),
        };
        g.in_adj.entry(edge.dst.0).or_default().push(in_entry);

        // Insert edge.
        g.edges.insert(raw_id, edge);

        Ok(())
    }

    /// Invalidate (remove) a graph from the cache so the next read reloads it.
    /// Only used for delete_graph — all other writes use write-through.
    pub(super) fn invalidate(&self, graph: &str) {
        if let Ok(mut loaded) = self.loaded.write() {
            loaded.remove(graph);
        }
        let _ = self.cache.delete_graph(graph);
    }
}
