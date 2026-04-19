// writes/nodes/properties.rs -- Node property mutations (SET / REPLACE / REMOVE).

use indexmap::IndexMap;

use cypher_core::error::StorageError;
use cypher_core::serde_value::props_to_json;
use cypher_core::storage::NodeId;
use cypher_core::types::Value;

use crate::keys::{key_node, key_node_write_lock};
use crate::lock::WRITE_LOCK_TTL_MS;
use crate::validation::validate_property_key;
use crate::RedisGraphStore;

use super::validate_property_keys;

impl RedisGraphStore {
    /// Merge additional properties into an existing node (SET semantics).
    ///
    /// Reads the current node, deindexes old property values, merges new
    /// properties on top, writes the merged set back, and reindexes.
    pub(crate) fn set_node_properties_impl(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        validate_property_keys(&properties)?;
        self.with_write_lock(
            graph,
            self.kp(key_node_write_lock(graph, id.0)),
            WRITE_LOCK_TTL_MS,
            3,
            50,
            |conn| {
                self.require_graph_on(conn.as_mut(), graph)?;
                let node = self
                    .read_node(conn.as_mut(), graph, id.0)?
                    .ok_or(StorageError::NodeNotFound { id: id.0 })?;
                // Deindex old property values before merging.
                self.deindex_node_properties(conn, graph, id.0, &node.labels, &node.properties)?;
                conn.renew_lock()?;
                let mut merged = node.properties;
                merged.extend(properties);
                let props_json = props_to_json(&merged)?;
                redis::cmd("HSET")
                    .arg(self.kp(key_node(graph, id.0)))
                    .arg("props")
                    .arg(&props_json)
                    .query::<()>(conn.as_mut())
                    .map_err(crate::backend_storage_error)?;
                // Reindex with new values.
                self.index_node_properties(conn, graph, id.0, &node.labels, &merged)?;
                Ok(())
            },
        )
    }

    /// Replace all properties on an existing node (REPLACE semantics).
    ///
    /// Unlike `set_node_properties_impl`, this discards all existing properties
    /// and replaces them entirely with the provided map.
    pub(crate) fn replace_node_properties_impl(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        validate_property_keys(&properties)?;
        self.with_write_lock(
            graph,
            self.kp(key_node_write_lock(graph, id.0)),
            WRITE_LOCK_TTL_MS,
            3,
            50,
            |conn| {
                self.require_graph_on(conn.as_mut(), graph)?;
                let node = self
                    .read_node(conn.as_mut(), graph, id.0)?
                    .ok_or(StorageError::NodeNotFound { id: id.0 })?;
                // Deindex old values.
                self.deindex_node_properties(conn, graph, id.0, &node.labels, &node.properties)?;
                conn.renew_lock()?;
                let props_json = props_to_json(&properties)?;
                redis::cmd("HSET")
                    .arg(self.kp(key_node(graph, id.0)))
                    .arg("props")
                    .arg(&props_json)
                    .query::<()>(conn.as_mut())
                    .map_err(crate::backend_storage_error)?;
                // Index new values.
                self.index_node_properties(conn, graph, id.0, &node.labels, &properties)?;
                Ok(())
            },
        )
    }

    /// Remove a single property key from a node.
    ///
    /// Deindexes all current properties, removes the specified key, writes
    /// the remaining properties back, and reindexes.
    pub(crate) fn remove_node_property_impl(
        &self,
        graph: &str,
        id: NodeId,
        key: &str,
    ) -> Result<(), StorageError> {
        validate_property_key(key)?;
        self.with_write_lock(
            graph,
            self.kp(key_node_write_lock(graph, id.0)),
            WRITE_LOCK_TTL_MS,
            3,
            50,
            |conn| {
                self.require_graph_on(conn.as_mut(), graph)?;
                let node = self
                    .read_node(conn.as_mut(), graph, id.0)?
                    .ok_or(StorageError::NodeNotFound { id: id.0 })?;
                // Deindex old values before removal.
                self.deindex_node_properties(conn, graph, id.0, &node.labels, &node.properties)?;
                conn.renew_lock()?;
                let mut props = node.properties;
                props.shift_remove(key);
                let props_json = props_to_json(&props)?;
                redis::cmd("HSET")
                    .arg(self.kp(key_node(graph, id.0)))
                    .arg("props")
                    .arg(&props_json)
                    .query::<()>(conn.as_mut())
                    .map_err(crate::backend_storage_error)?;
                // Reindex with remaining values.
                self.index_node_properties(conn, graph, id.0, &node.labels, &props)?;
                Ok(())
            },
        )
    }
}
