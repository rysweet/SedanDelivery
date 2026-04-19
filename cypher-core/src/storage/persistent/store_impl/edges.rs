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
        let id = self
            .inner
            .create_edge(graph, src, dst, rel_type.clone(), properties.clone())?;
        self.append_wal(&WalEntry::CreateEdge {
            graph: graph.to_string(),
            src: src.0,
            dst: dst.0,
            rel_type,
            properties: props_to_json(&properties),
            id: id.0,
        })?;
        Ok(id)
    }

    pub(super) fn set_edge_properties_impl(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.inner
            .set_edge_properties(graph, id, properties.clone())?;
        self.append_wal(&WalEntry::SetEdgeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json(&properties),
        })?;
        Ok(())
    }

    pub(super) fn replace_edge_properties_impl(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.inner
            .replace_edge_properties(graph, id, properties.clone())?;
        self.append_wal(&WalEntry::ReplaceEdgeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json(&properties),
        })?;
        Ok(())
    }

    pub(super) fn remove_edge_property_impl(
        &self,
        graph: &str,
        id: EdgeId,
        key: &str,
    ) -> Result<(), StorageError> {
        self.inner.remove_edge_property(graph, id, key)?;
        self.append_wal(&WalEntry::RemoveEdgeProperty {
            graph: graph.to_string(),
            id: id.0,
            key: key.to_string(),
        })?;
        Ok(())
    }

    pub(super) fn delete_edge_impl(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.inner.delete_edge(graph, id)?;
        self.append_wal(&WalEntry::DeleteEdge {
            graph: graph.to_string(),
            id: id.0,
        })?;
        Ok(())
    }
}
