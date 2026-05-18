//! Node-write inherent helpers for [`PersistentGraphStore`].

use indexmap::IndexMap;

use super::super::wal::{props_to_json, WalEntry};
use super::super::PersistentGraphStore;
use crate::error::StorageError;
use crate::storage::{GraphStore, NodeId};
use crate::types::Value;

impl PersistentGraphStore {
    pub(super) fn create_node_impl(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        // Write-ahead: allocate ID, write WAL, then apply mutation.
        let id = self.inner.allocate_node_id(graph)?;
        self.append_wal(&WalEntry::CreateNode {
            graph: graph.to_string(),
            labels: labels.clone(),
            properties: props_to_json(&properties),
            id: id.0,
        })?;
        self.inner
            .create_node_with_id(graph, id, labels, properties)?;
        Ok(id)
    }

    pub(super) fn set_node_properties_impl(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.append_wal(&WalEntry::SetNodeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json(&properties),
        })?;
        self.inner.set_node_properties(graph, id, properties)?;
        Ok(())
    }

    pub(super) fn replace_node_properties_impl(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.append_wal(&WalEntry::ReplaceNodeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json(&properties),
        })?;
        self.inner.replace_node_properties(graph, id, properties)?;
        Ok(())
    }

    pub(super) fn remove_node_property_impl(
        &self,
        graph: &str,
        id: NodeId,
        key: &str,
    ) -> Result<(), StorageError> {
        self.append_wal(&WalEntry::RemoveNodeProperty {
            graph: graph.to_string(),
            id: id.0,
            key: key.to_string(),
        })?;
        self.inner.remove_node_property(graph, id, key)?;
        Ok(())
    }

    pub(super) fn add_node_labels_impl(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.append_wal(&WalEntry::AddNodeLabels {
            graph: graph.to_string(),
            id: id.0,
            labels: labels.to_vec(),
        })?;
        self.inner.add_node_labels(graph, id, labels)?;
        Ok(())
    }

    pub(super) fn remove_node_labels_impl(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.append_wal(&WalEntry::RemoveNodeLabels {
            graph: graph.to_string(),
            id: id.0,
            labels: labels.to_vec(),
        })?;
        self.inner.remove_node_labels(graph, id, labels)?;
        Ok(())
    }

    pub(super) fn delete_node_impl(
        &self,
        graph: &str,
        id: NodeId,
        detach: bool,
    ) -> Result<(), StorageError> {
        if detach {
            let out_ids = self.inner.out_edge_ids(graph, id)?;
            let in_ids = self.inner.in_edge_ids(graph, id)?;
            // Write-ahead: log all edge deletions and node deletion before mutation.
            for eid in &out_ids {
                self.append_wal(&WalEntry::DeleteEdge {
                    graph: graph.to_string(),
                    id: eid.0,
                })?;
            }
            for eid in &in_ids {
                self.append_wal(&WalEntry::DeleteEdge {
                    graph: graph.to_string(),
                    id: eid.0,
                })?;
            }
            self.append_wal(&WalEntry::DeleteNode {
                graph: graph.to_string(),
                id: id.0,
            })?;
            self.inner.delete_node(graph, id, true)?;
        } else {
            self.append_wal(&WalEntry::DeleteNode {
                graph: graph.to_string(),
                id: id.0,
            })?;
            self.inner.delete_node(graph, id, false)?;
        }
        Ok(())
    }
}
