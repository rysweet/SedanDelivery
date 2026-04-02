//! Write-through cached graph store.
//!
//! Wraps any `GraphStore` backend with an `InMemoryGraphStore` read cache.
//! On first access to a graph, the entire graph is lazily loaded from the
//! backend into the cache. Subsequent reads are served from cache.
//! Writes go to the backend first, then the cache is invalidated so the
//! next read reloads from the backend (invalidate-on-write strategy).
//!
//! This avoids ID synchronization issues between backend and cache by
//! treating the cache as a full-graph snapshot that is rebuilt on demand.

use std::any::Any;
use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use indexmap::IndexMap;

use crate::error::StorageError;
use crate::storage::{
    validate_graph_name, AdjEntry, ConstraintDescriptor, ConstraintType, EdgeData, EdgeId,
    GraphStats, GraphStore, InMemoryGraphStore, IndexDescriptor, NodeData, NodeId, RangeQuery,
    ReadConsistency,
};
use crate::types::Value;

/// A write-through cached graph store.
///
/// - Reads: lazy-load full graph into `InMemoryGraphStore` cache on first access,
///   then serve from cache.
/// - Writes: apply to backend first, then invalidate the cached graph so the
///   next read reloads fresh data.
pub struct CachedGraphStore {
    backend: Arc<dyn GraphStore>,
    cache: InMemoryGraphStore,
    /// Set of graph names that have been loaded into the cache.
    loaded: RwLock<HashSet<String>>,
}

impl std::fmt::Debug for CachedGraphStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let loaded = self
            .loaded
            .read()
            .map(|g| g.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        f.debug_struct("CachedGraphStore")
            .field("loaded_graphs", &loaded)
            .finish()
    }
}

impl CachedGraphStore {
    /// Create a new cached store wrapping the given backend.
    pub fn new(backend: Arc<dyn GraphStore>) -> Self {
        Self {
            backend,
            cache: InMemoryGraphStore::new(),
            loaded: RwLock::new(HashSet::new()),
        }
    }

    /// Eagerly warm the cache for a specific graph.
    pub fn cache_graph(&self, graph: &str) -> Result<(), StorageError> {
        self.load_graph(graph)
    }

    /// Check if a graph is currently cached.
    pub fn is_cached(&self, graph: &str) -> bool {
        self.loaded
            .read()
            .map(|s| s.contains(graph))
            .unwrap_or(false)
    }

    /// Ensure the named graph is loaded in the cache.
    /// If already loaded, this is a no-op.
    fn ensure_loaded(&self, graph: &str) -> Result<(), StorageError> {
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
    fn load_graph(&self, graph: &str) -> Result<(), StorageError> {
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
    fn inject_node_into_cache(&self, graph: &str, node: NodeData) -> Result<(), StorageError> {
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
    fn inject_edge_into_cache(&self, graph: &str, edge: EdgeData) -> Result<(), StorageError> {
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
    fn invalidate(&self, graph: &str) {
        if let Ok(mut loaded) = self.loaded.write() {
            loaded.remove(graph);
        }
        let _ = self.cache.delete_graph(graph);
    }

    /// Get a mutable reference to a graph's inner data.
    fn get_graph_mut<'a>(
        graphs: &'a mut std::collections::HashMap<String, crate::storage::mem::GraphInner>,
        graph: &str,
    ) -> Result<&'a mut crate::storage::mem::GraphInner, StorageError> {
        graphs
            .get_mut(graph)
            .ok_or_else(|| StorageError::GraphNotFound {
                name: graph.to_string(),
            })
    }

    /// Update node properties in cache (merge semantics).
    fn update_node_props_in_cache(
        &self,
        graph: &str,
        id: NodeId,
        properties: &IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut guard = self
            .cache
            .inner
            .write()
            .map_err(|e| StorageError::backend(format!("lock poisoned: {e}")))?;
        let graphs = Arc::make_mut(&mut guard);
        let g = Self::get_graph_mut(graphs, graph)?;
        if let Some(node) = g.nodes.get(&id.0) {
            let old_labels = node.labels.clone();
            let old_props = node.properties.clone();
            g.index_remove(id.0, &old_labels, &old_props);
            let node = g.nodes.get_mut(&id.0).unwrap();
            for (k, v) in properties {
                node.properties.insert(k.clone(), v.clone());
            }
            let new_labels = node.labels.clone();
            let new_props = node.properties.clone();
            g.index_insert(id.0, &new_labels, &new_props);
        }
        Ok(())
    }

    /// Replace all node properties in cache.
    fn replace_node_props_in_cache(
        &self,
        graph: &str,
        id: NodeId,
        properties: &IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut guard = self
            .cache
            .inner
            .write()
            .map_err(|e| StorageError::backend(format!("lock poisoned: {e}")))?;
        let graphs = Arc::make_mut(&mut guard);
        let g = Self::get_graph_mut(graphs, graph)?;
        if let Some(node) = g.nodes.get(&id.0) {
            let old_labels = node.labels.clone();
            let old_props = node.properties.clone();
            g.index_remove(id.0, &old_labels, &old_props);
            let node = g.nodes.get_mut(&id.0).unwrap();
            node.properties = properties.clone();
            let new_labels = node.labels.clone();
            let new_props = node.properties.clone();
            g.index_insert(id.0, &new_labels, &new_props);
        }
        Ok(())
    }

    /// Update edge properties in cache (merge semantics).
    fn update_edge_props_in_cache(
        &self,
        graph: &str,
        id: EdgeId,
        properties: &IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut guard = self
            .cache
            .inner
            .write()
            .map_err(|e| StorageError::backend(format!("lock poisoned: {e}")))?;
        let graphs = Arc::make_mut(&mut guard);
        let g = Self::get_graph_mut(graphs, graph)?;
        if let Some(edge) = g.edges.get_mut(&id.0) {
            for (k, v) in properties {
                edge.properties.insert(k.clone(), v.clone());
            }
        }
        Ok(())
    }

    /// Replace all edge properties in cache.
    fn replace_edge_props_in_cache(
        &self,
        graph: &str,
        id: EdgeId,
        properties: &IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut guard = self
            .cache
            .inner
            .write()
            .map_err(|e| StorageError::backend(format!("lock poisoned: {e}")))?;
        let graphs = Arc::make_mut(&mut guard);
        let g = Self::get_graph_mut(graphs, graph)?;
        if let Some(edge) = g.edges.get_mut(&id.0) {
            edge.properties = properties.clone();
        }
        Ok(())
    }

    /// Remove a single property from a node in cache.
    fn remove_node_prop_in_cache(
        &self,
        graph: &str,
        id: NodeId,
        key: &str,
    ) -> Result<(), StorageError> {
        let mut guard = self
            .cache
            .inner
            .write()
            .map_err(|e| StorageError::backend(format!("lock poisoned: {e}")))?;
        let graphs = Arc::make_mut(&mut guard);
        let g = Self::get_graph_mut(graphs, graph)?;
        if let Some(node) = g.nodes.get(&id.0) {
            let old_labels = node.labels.clone();
            let old_props = node.properties.clone();
            g.index_remove(id.0, &old_labels, &old_props);
            let node = g.nodes.get_mut(&id.0).unwrap();
            node.properties.shift_remove(key);
            let new_labels = node.labels.clone();
            let new_props = node.properties.clone();
            g.index_insert(id.0, &new_labels, &new_props);
        }
        Ok(())
    }

    /// Remove a single property from an edge in cache.
    fn remove_edge_prop_in_cache(
        &self,
        graph: &str,
        id: EdgeId,
        key: &str,
    ) -> Result<(), StorageError> {
        let mut guard = self
            .cache
            .inner
            .write()
            .map_err(|e| StorageError::backend(format!("lock poisoned: {e}")))?;
        let graphs = Arc::make_mut(&mut guard);
        let g = Self::get_graph_mut(graphs, graph)?;
        if let Some(edge) = g.edges.get_mut(&id.0) {
            edge.properties.shift_remove(key);
        }
        Ok(())
    }

    /// Add labels to a node in cache.
    fn add_labels_in_cache(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let mut guard = self
            .cache
            .inner
            .write()
            .map_err(|e| StorageError::backend(format!("lock poisoned: {e}")))?;
        let graphs = Arc::make_mut(&mut guard);
        let g = Self::get_graph_mut(graphs, graph)?;
        // First update label index (doesn't borrow nodes).
        for label in labels {
            g.label_index.entry(label.clone()).or_default().insert(id.0);
        }
        // Then update the node's labels.
        if let Some(node) = g.nodes.get_mut(&id.0) {
            for label in labels {
                if !node.labels.contains(label) {
                    node.labels.push(label.clone());
                }
            }
        }
        // Re-index: extract data first to avoid borrow conflict.
        if let Some(node) = g.nodes.get(&id.0) {
            let lbls = node.labels.clone();
            let props = node.properties.clone();
            g.index_insert(id.0, &lbls, &props);
        }
        Ok(())
    }

    /// Remove labels from a node in cache.
    fn remove_labels_in_cache(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let mut guard = self
            .cache
            .inner
            .write()
            .map_err(|e| StorageError::backend(format!("lock poisoned: {e}")))?;
        let graphs = Arc::make_mut(&mut guard);
        let g = Self::get_graph_mut(graphs, graph)?;
        // Extract old state for index removal.
        let (old_labels, old_props) = match g.nodes.get(&id.0) {
            Some(node) => (node.labels.clone(), node.properties.clone()),
            None => return Ok(()),
        };
        g.index_remove(id.0, &old_labels, &old_props);
        // Update the node.
        if let Some(node) = g.nodes.get_mut(&id.0) {
            for label in labels {
                node.labels.retain(|l| l != label);
                if let Some(set) = g.label_index.get_mut(label) {
                    set.remove(&id.0);
                }
            }
        }
        // Re-index with remaining labels.
        if let Some(node) = g.nodes.get(&id.0) {
            let new_labels = node.labels.clone();
            let new_props = node.properties.clone();
            g.index_insert(id.0, &new_labels, &new_props);
        }
        Ok(())
    }

    /// Remove a node from cache (with optional detach of edges).
    fn remove_node_from_cache(
        &self,
        graph: &str,
        id: NodeId,
        detach: bool,
    ) -> Result<(), StorageError> {
        let mut guard = self
            .cache
            .inner
            .write()
            .map_err(|e| StorageError::backend(format!("lock poisoned: {e}")))?;
        let graphs = Arc::make_mut(&mut guard);
        let g = Self::get_graph_mut(graphs, graph)?;

        if let Some(node) = g.nodes.remove(&id.0) {
            for label in &node.labels {
                if let Some(set) = g.label_index.get_mut(label) {
                    set.remove(&id.0);
                }
            }
            g.index_remove(id.0, &node.labels, &node.properties);

            if detach {
                if let Some(out_entries) = g.out_adj.remove(&id.0) {
                    for entry in &out_entries {
                        if let Some(edge) = g.edges.remove(&entry.edge_id.0) {
                            if let Some(set) = g.reltype_index.get_mut(&edge.rel_type) {
                                set.remove(&entry.edge_id.0);
                            }
                        }
                        if let Some(in_list) = g.in_adj.get_mut(&entry.neighbor_id.0) {
                            in_list.retain(|e| e.edge_id != entry.edge_id);
                        }
                    }
                }
                if let Some(in_entries) = g.in_adj.remove(&id.0) {
                    for entry in &in_entries {
                        if let Some(edge) = g.edges.remove(&entry.edge_id.0) {
                            if let Some(set) = g.reltype_index.get_mut(&edge.rel_type) {
                                set.remove(&entry.edge_id.0);
                            }
                        }
                        if let Some(out_list) = g.out_adj.get_mut(&entry.neighbor_id.0) {
                            out_list.retain(|e| e.edge_id != entry.edge_id);
                        }
                    }
                }
            } else {
                g.out_adj.remove(&id.0);
                g.in_adj.remove(&id.0);
            }
        }
        Ok(())
    }

    /// Remove an edge from cache.
    fn remove_edge_from_cache(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        let mut guard = self
            .cache
            .inner
            .write()
            .map_err(|e| StorageError::backend(format!("lock poisoned: {e}")))?;
        let graphs = Arc::make_mut(&mut guard);
        let g = Self::get_graph_mut(graphs, graph)?;

        if let Some(edge) = g.edges.remove(&id.0) {
            if let Some(set) = g.reltype_index.get_mut(&edge.rel_type) {
                set.remove(&id.0);
            }
            if let Some(out_list) = g.out_adj.get_mut(&edge.src.0) {
                out_list.retain(|e| e.edge_id != id);
            }
            if let Some(in_list) = g.in_adj.get_mut(&edge.dst.0) {
                in_list.retain(|e| e.edge_id != id);
            }
        }
        Ok(())
    }
}

impl GraphStore for CachedGraphStore {
    // ── Single-item reads ────────────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.get_node(graph, id)
    }

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.get_edge(graph, id)
    }

    // ── Batch reads ──────────────────────────────────────────────────────────

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.get_nodes_batch(graph, ids)
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.get_edges_batch(graph, ids)
    }

    // ── Label/type index reads ───────────────────────────────────────────────

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.find_nodes_by_label(graph, label)
    }

    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache
            .find_nodes_by_label_and_property(graph, label, property, range)
    }

    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.find_edges_by_type(graph, rel_type)
    }

    // ── Adjacency reads ──────────────────────────────────────────────────────

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.out_edges(graph, node_id)
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.in_edges(graph, node_id)
    }

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.out_edges_by_type(graph, node_id, rel_types)
    }

    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.in_edges_by_type(graph, node_id, rel_types)
    }

    fn out_edges_by_type_batch(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache
            .out_edges_by_type_batch(graph, node_ids, rel_types)
    }

    fn in_edges_by_type_batch(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache
            .in_edges_by_type_batch(graph, node_ids, rel_types)
    }

    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.out_edge_ids(graph, node_id)
    }

    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.in_edge_ids(graph, node_id)
    }

    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.all_node_ids(graph)
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.all_edge_ids(graph)
    }

    // ── Write operations (write-through + invalidate) ────────────────────────

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        let id = self
            .backend
            .create_node(graph, labels.clone(), properties.clone())?;
        if self.is_cached(graph) {
            let node = NodeData {
                id,
                labels,
                properties,
            };
            self.inject_node_into_cache(graph, node)?;
        }
        Ok(id)
    }

    fn batch_create_nodes(
        &self,
        graph: &str,
        nodes: Vec<(Vec<String>, IndexMap<String, Value>)>,
    ) -> Result<Vec<NodeId>, StorageError> {
        let ids = self.backend.batch_create_nodes(graph, nodes.clone())?;
        if self.is_cached(graph) {
            for (id, (labels, properties)) in ids.iter().zip(nodes) {
                let node = NodeData {
                    id: *id,
                    labels,
                    properties,
                };
                self.inject_node_into_cache(graph, node)?;
            }
        }
        Ok(ids)
    }

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        let id = self
            .backend
            .create_edge(graph, src, dst, rel_type.clone(), properties.clone())?;
        if self.is_cached(graph) {
            let edge = EdgeData {
                id,
                src,
                dst,
                rel_type,
                properties,
            };
            self.inject_edge_into_cache(graph, edge)?;
        }
        Ok(id)
    }

    fn batch_create_edges(
        &self,
        graph: &str,
        edges: Vec<(NodeId, NodeId, String, IndexMap<String, Value>)>,
    ) -> Result<Vec<EdgeId>, StorageError> {
        let ids = self.backend.batch_create_edges(graph, edges.clone())?;
        if self.is_cached(graph) {
            for (id, (src, dst, rel_type, properties)) in ids.iter().zip(edges) {
                let edge = EdgeData {
                    id: *id,
                    src,
                    dst,
                    rel_type,
                    properties,
                };
                self.inject_edge_into_cache(graph, edge)?;
            }
        }
        Ok(ids)
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.backend
            .set_node_properties(graph, id, properties.clone())?;
        if self.is_cached(graph) {
            self.update_node_props_in_cache(graph, id, &properties)?;
        }
        Ok(())
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.backend
            .replace_node_properties(graph, id, properties.clone())?;
        if self.is_cached(graph) {
            self.replace_node_props_in_cache(graph, id, &properties)?;
        }
        Ok(())
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.backend
            .set_edge_properties(graph, id, properties.clone())?;
        if self.is_cached(graph) {
            self.update_edge_props_in_cache(graph, id, &properties)?;
        }
        Ok(())
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.backend
            .replace_edge_properties(graph, id, properties.clone())?;
        if self.is_cached(graph) {
            self.replace_edge_props_in_cache(graph, id, &properties)?;
        }
        Ok(())
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        self.backend.remove_node_property(graph, id, key)?;
        if self.is_cached(graph) {
            self.remove_node_prop_in_cache(graph, id, key)?;
        }
        Ok(())
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        self.backend.remove_edge_property(graph, id, key)?;
        if self.is_cached(graph) {
            self.remove_edge_prop_in_cache(graph, id, key)?;
        }
        Ok(())
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.backend.add_node_labels(graph, id, labels)?;
        if self.is_cached(graph) {
            self.add_labels_in_cache(graph, id, labels)?;
        }
        Ok(())
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.backend.remove_node_labels(graph, id, labels)?;
        if self.is_cached(graph) {
            self.remove_labels_in_cache(graph, id, labels)?;
        }
        Ok(())
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        self.backend.delete_node(graph, id, detach)?;
        if self.is_cached(graph) {
            self.remove_node_from_cache(graph, id, detach)?;
        }
        Ok(())
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.backend.delete_edge(graph, id)?;
        if self.is_cached(graph) {
            self.remove_edge_from_cache(graph, id)?;
        }
        Ok(())
    }

    // ── Index management ─────────────────────────────────────────────────────

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.list_indexes(graph)
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.backend.create_index(graph, label, property)?;
        if self.is_cached(graph) {
            self.cache.create_index(graph, label, property)?;
        }
        Ok(())
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.backend.drop_index(graph, label, property)?;
        if self.is_cached(graph) {
            self.cache.drop_index(graph, label, property)?;
        }
        Ok(())
    }

    // ── Constraint management ────────────────────────────────────────────────

    fn list_constraints(&self, graph: &str) -> Result<Vec<ConstraintDescriptor>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.list_constraints(graph)
    }

    fn create_constraint(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        constraint_type: ConstraintType,
    ) -> Result<(), StorageError> {
        self.backend
            .create_constraint(graph, label, property, constraint_type.clone())?;
        if self.is_cached(graph) {
            self.cache
                .create_constraint(graph, label, property, constraint_type)?;
        }
        Ok(())
    }

    fn drop_constraint(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StorageError> {
        self.backend.drop_constraint(graph, label, property)?;
        if self.is_cached(graph) {
            self.cache.drop_constraint(graph, label, property)?;
        }
        Ok(())
    }

    // ── Graph administration ─────────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        validate_graph_name(name)?;
        self.backend.create_graph(name)?;
        // Don't auto-cache empty graphs; they'll be loaded on first access.
        Ok(())
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        self.backend.delete_graph(name)?;
        self.invalidate(name);
        Ok(())
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        // Check backend directly — don't require full load.
        self.backend.graph_exists(name)
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        self.backend.list_graphs()
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.graph_stats(graph)
    }

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        self.ensure_loaded(graph)?;
        self.cache.all_label_names(graph)
    }

    fn with_read_consistency(&self, _consistency: ReadConsistency) -> Option<Arc<dyn GraphStore>> {
        // CachedGraphStore doesn't support topology-aware reads.
        None
    }

    // ── Transaction support ──────────────────────────────────────────────────

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn tx_snapshot(&self, _graphs: &[&str]) -> Result<Option<Box<dyn GraphStore>>, StorageError> {
        // Delegate to backend — cache is not transactional.
        Ok(None)
    }

    fn tx_commit(&self, _snapshot: &dyn GraphStore) -> Result<(), StorageError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_backend() -> Arc<InMemoryGraphStore> {
        Arc::new(InMemoryGraphStore::new())
    }

    #[test]
    fn write_then_read_through_cache() {
        let backend = make_backend();
        let cached = CachedGraphStore::new(backend.clone());

        // Create graph and node via cached store.
        cached.create_graph("test").unwrap();
        let mut props = IndexMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let nid = cached
            .create_node("test", vec!["Person".to_string()], props)
            .unwrap();

        // Read back through cache.
        let node = cached.get_node("test", nid).unwrap().unwrap();
        assert_eq!(node.labels, vec!["Person".to_string()]);
        assert_eq!(
            node.properties.get("name"),
            Some(&Value::Str("Alice".into()))
        );
    }

    #[test]
    fn write_through_visible_in_backend() {
        let backend = make_backend();
        let cached = CachedGraphStore::new(backend.clone());

        cached.create_graph("g").unwrap();
        let mut props = IndexMap::new();
        props.insert("x".to_string(), Value::Int(42));
        let nid = cached
            .create_node("g", vec!["Thing".to_string()], props)
            .unwrap();

        // Verify data is in the backend directly.
        let backend_node = backend.get_node("g", nid).unwrap().unwrap();
        assert_eq!(backend_node.properties.get("x"), Some(&Value::Int(42)));
    }

    #[test]
    fn cache_miss_falls_through_to_backend() {
        let backend = make_backend();

        // Write directly to backend (bypassing the cache).
        backend.create_graph("g").unwrap();
        let mut props = IndexMap::new();
        props.insert("val".to_string(), Value::Int(99));
        let nid = backend
            .create_node("g", vec!["Item".to_string()], props)
            .unwrap();

        // Now create CachedGraphStore — cache is cold.
        let cached = CachedGraphStore::new(backend.clone());

        // Read should load from backend into cache.
        let node = cached.get_node("g", nid).unwrap().unwrap();
        assert_eq!(node.properties.get("val"), Some(&Value::Int(99)));
        assert_eq!(node.labels, vec!["Item".to_string()]);

        // Verify it's now cached.
        assert!(cached.is_cached("g"));
    }

    #[test]
    fn edges_cached_correctly() {
        let backend = make_backend();
        backend.create_graph("g").unwrap();
        let n1 = backend
            .create_node("g", vec!["A".into()], IndexMap::new())
            .unwrap();
        let n2 = backend
            .create_node("g", vec!["B".into()], IndexMap::new())
            .unwrap();
        let eid = backend
            .create_edge("g", n1, n2, "KNOWS".into(), IndexMap::new())
            .unwrap();

        let cached = CachedGraphStore::new(backend.clone());

        // Read edge through cache.
        let edge = cached.get_edge("g", eid).unwrap().unwrap();
        assert_eq!(edge.rel_type, "KNOWS");
        assert_eq!(edge.src, n1);
        assert_eq!(edge.dst, n2);

        // Verify adjacency.
        let out = cached.out_edges("g", n1).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].neighbor_id, n2);

        let inc = cached.in_edges("g", n2).unwrap();
        assert_eq!(inc.len(), 1);
        assert_eq!(inc[0].neighbor_id, n1);
    }

    #[test]
    fn write_through_keeps_cache_valid() {
        let backend = make_backend();
        let cached = CachedGraphStore::new(backend.clone());

        cached.create_graph("g").unwrap();
        // Cache is not loaded yet (create_graph doesn't auto-cache).
        assert!(!cached.is_cached("g"));

        // First read loads the cache.
        let _ = cached.all_node_ids("g").unwrap();
        assert!(cached.is_cached("g"));

        // Create node — write-through keeps cache valid.
        let nid = cached
            .create_node("g", vec!["X".into()], IndexMap::new())
            .unwrap();
        assert!(cached.is_cached("g"));

        // Node is visible through cache without reload.
        let node = cached.get_node("g", nid).unwrap();
        assert!(node.is_some());

        // Property update — cache stays valid.
        let mut props = IndexMap::new();
        props.insert("key".to_string(), Value::Str("val".into()));
        cached.set_node_properties("g", nid, props).unwrap();
        assert!(cached.is_cached("g"));

        // Property visible through cache.
        let node = cached.get_node("g", nid).unwrap().unwrap();
        assert_eq!(node.properties.get("key"), Some(&Value::Str("val".into())));
    }

    #[test]
    fn find_nodes_by_label_cached() {
        let backend = make_backend();
        backend.create_graph("g").unwrap();
        let _ = backend
            .create_node("g", vec!["Person".into()], IndexMap::new())
            .unwrap();
        let _ = backend
            .create_node("g", vec!["Person".into()], IndexMap::new())
            .unwrap();
        let _ = backend
            .create_node("g", vec!["Car".into()], IndexMap::new())
            .unwrap();

        let cached = CachedGraphStore::new(backend.clone());
        let people = cached.find_nodes_by_label("g", "Person").unwrap();
        assert_eq!(people.len(), 2);
    }

    #[test]
    fn graph_admin_delegates_to_backend() {
        let backend = make_backend();
        let cached = CachedGraphStore::new(backend.clone());

        cached.create_graph("a").unwrap();
        cached.create_graph("b").unwrap();

        assert!(cached.graph_exists("a").unwrap());
        assert!(cached.graph_exists("b").unwrap());

        let mut graphs = cached.list_graphs().unwrap();
        graphs.sort();
        assert_eq!(graphs, vec!["a".to_string(), "b".to_string()]);

        cached.delete_graph("a").unwrap();
        assert!(!cached.graph_exists("a").unwrap());
    }

    #[test]
    fn cache_graph_warms_cache() {
        let backend = make_backend();
        backend.create_graph("g").unwrap();
        backend
            .create_node("g", vec!["N".into()], IndexMap::new())
            .unwrap();

        let cached = CachedGraphStore::new(backend.clone());
        assert!(!cached.is_cached("g"));

        cached.cache_graph("g").unwrap();
        assert!(cached.is_cached("g"));
    }

    #[test]
    fn delete_node_write_through() {
        let backend = make_backend();
        let cached = CachedGraphStore::new(backend.clone());

        cached.create_graph("g").unwrap();
        let nid = cached
            .create_node("g", vec!["X".into()], IndexMap::new())
            .unwrap();

        // Node exists.
        assert!(cached.get_node("g", nid).unwrap().is_some());

        // Delete through cache.
        cached.delete_node("g", nid, false).unwrap();

        // Should be gone from backend.
        assert!(backend.get_node("g", nid).unwrap().is_none());

        // And from cache (after reload).
        assert!(cached.get_node("g", nid).unwrap().is_none());
    }
}
