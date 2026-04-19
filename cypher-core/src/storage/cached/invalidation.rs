//! In-cache mutation helpers used by the write-through paths.
//!
//! Each helper assumes the caller has already applied the write to the
//! backend; its job is to mirror that change into the cached
//! `InMemoryGraphStore` so subsequent reads stay coherent.

use std::sync::Arc;

use indexmap::IndexMap;

use crate::error::StorageError;
use crate::storage::{EdgeId, NodeId};
use crate::types::Value;

use super::CachedGraphStore;

impl CachedGraphStore {
    /// Get a mutable reference to a graph's inner data.
    pub(super) fn get_graph_mut<'a>(
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
    pub(super) fn update_node_props_in_cache(
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
    pub(super) fn replace_node_props_in_cache(
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
    pub(super) fn update_edge_props_in_cache(
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
    pub(super) fn replace_edge_props_in_cache(
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
    pub(super) fn remove_node_prop_in_cache(
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
    pub(super) fn remove_edge_prop_in_cache(
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
    pub(super) fn add_labels_in_cache(
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
    pub(super) fn remove_labels_in_cache(
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
    pub(super) fn remove_node_from_cache(
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
    pub(super) fn remove_edge_from_cache(
        &self,
        graph: &str,
        id: EdgeId,
    ) -> Result<(), StorageError> {
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
