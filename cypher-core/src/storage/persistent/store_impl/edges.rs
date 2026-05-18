//! Edge-write inherent helpers for [`PersistentGraphStore`].

use indexmap::IndexMap;

use super::super::wal::{props_to_json, WalEntry};
use super::super::PersistentGraphStore;
use crate::error::StorageError;
use crate::storage::{EdgeId, GraphStore, NodeId};
use crate::types::Value;

impl PersistentGraphStore {
    pub(super) fn create_edge_impl(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        // Write-ahead: allocate ID, write WAL, then apply mutation.
        let id = self.inner.allocate_edge_id(graph)?;
        self.append_wal(&WalEntry::CreateEdge {
            graph: graph.to_string(),
            src: src.0,
            dst: dst.0,
            rel_type: rel_type.clone(),
            properties: props_to_json(&properties),
            id: id.0,
        })?;
        self.inner
            .create_edge_with_id(graph, id, src, dst, rel_type, properties)?;
        Ok(id)
    }

    pub(super) fn set_edge_properties_impl(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.append_wal(&WalEntry::SetEdgeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json(&properties),
        })?;
        self.inner.set_edge_properties(graph, id, properties)?;
        Ok(())
    }

    pub(super) fn replace_edge_properties_impl(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.append_wal(&WalEntry::ReplaceEdgeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json(&properties),
        })?;
        self.inner.replace_edge_properties(graph, id, properties)?;
        Ok(())
    }

    pub(super) fn remove_edge_property_impl(
        &self,
        graph: &str,
        id: EdgeId,
        key: &str,
    ) -> Result<(), StorageError> {
        self.append_wal(&WalEntry::RemoveEdgeProperty {
            graph: graph.to_string(),
            id: id.0,
            key: key.to_string(),
        })?;
        self.inner.remove_edge_property(graph, id, key)?;
        Ok(())
    }

    pub(super) fn delete_edge_impl(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.append_wal(&WalEntry::DeleteEdge {
            graph: graph.to_string(),
            id: id.0,
        })?;
        self.inner.delete_edge(graph, id)?;
        Ok(())
    }
}
