//! Schema (index/constraint) and graph-administration inherent helpers
//! for [`CachedGraphStore`].
//!
//! Mutating helpers apply to the backend first, then mirror the change into
//! the cache. Graph-administration helpers operate on the backend directly
//! (no cache state required).

use crate::error::StorageError;
use crate::storage::{validate_graph_name, ConstraintType, GraphStore};

use super::CachedGraphStore;

impl CachedGraphStore {
    pub(super) fn schema_create_index(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StorageError> {
        self.backend.create_index(graph, label, property)?;
        if self.is_cached(graph) {
            self.cache.create_index(graph, label, property)?;
        }
        Ok(())
    }

    pub(super) fn schema_drop_index(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StorageError> {
        self.backend.drop_index(graph, label, property)?;
        if self.is_cached(graph) {
            self.cache.drop_index(graph, label, property)?;
        }
        Ok(())
    }

    pub(super) fn schema_create_constraint(
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

    pub(super) fn schema_drop_constraint(
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

    pub(super) fn admin_create_graph(&self, name: &str) -> Result<(), StorageError> {
        validate_graph_name(name)?;
        self.backend.create_graph(name)?;
        // Don't auto-cache empty graphs; they'll be loaded on first access.
        Ok(())
    }

    pub(super) fn admin_delete_graph(&self, name: &str) -> Result<(), StorageError> {
        self.backend.delete_graph(name)?;
        self.invalidate(name);
        Ok(())
    }

    pub(super) fn admin_graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        // Check backend directly — don't require full load.
        self.backend.graph_exists(name)
    }

    pub(super) fn admin_list_graphs(&self) -> Result<Vec<String>, StorageError> {
        self.backend.list_graphs()
    }
}
