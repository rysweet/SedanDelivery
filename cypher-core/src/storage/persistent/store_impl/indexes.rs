//! Graph-level and index/constraint write helpers for [`PersistentGraphStore`].

use super::super::wal::WalEntry;
use super::super::PersistentGraphStore;
use crate::error::StorageError;
use crate::storage::{ConstraintType, GraphStore};

impl PersistentGraphStore {
    pub(super) fn create_graph_impl(&self, name: &str) -> Result<(), StorageError> {
        self.inner.create_graph(name)?;
        self.append_wal(&WalEntry::CreateGraph {
            name: name.to_string(),
        })?;
        Ok(())
    }

    pub(super) fn delete_graph_impl(&self, name: &str) -> Result<(), StorageError> {
        self.inner.delete_graph(name)?;
        self.append_wal(&WalEntry::DeleteGraph {
            name: name.to_string(),
        })?;
        Ok(())
    }

    pub(super) fn create_index_impl(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StorageError> {
        self.inner.create_index(graph, label, property)?;
        self.append_wal(&WalEntry::CreateIndex {
            graph: graph.to_string(),
            label: label.to_string(),
            property: property.to_string(),
        })?;
        Ok(())
    }

    pub(super) fn drop_index_impl(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StorageError> {
        self.inner.drop_index(graph, label, property)?;
        self.append_wal(&WalEntry::DropIndex {
            graph: graph.to_string(),
            label: label.to_string(),
            property: property.to_string(),
        })?;
        Ok(())
    }

    pub(super) fn create_constraint_impl(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        constraint_type: ConstraintType,
    ) -> Result<(), StorageError> {
        self.inner
            .create_constraint(graph, label, property, constraint_type)
    }

    pub(super) fn drop_constraint_impl(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StorageError> {
        self.inner.drop_constraint(graph, label, property)
    }
}
