// writes/edges/properties.rs -- SET / REPLACE / REMOVE property operations on edges.

use indexmap::IndexMap;

use cypher_core::error::StorageError;
use cypher_core::serde_value::props_to_json;
use cypher_core::storage::EdgeId;
use cypher_core::types::Value;

use crate::keys::{key_edge, key_edge_id_write_lock};
use crate::lock::WRITE_LOCK_TTL_MS;
use crate::validation::validate_property_key;
use crate::RedisGraphStore;

use super::validate_property_keys;

impl RedisGraphStore {
    /// Merge additional properties into an existing edge (SET semantics).
    ///
    /// Reads the current edge, merges new properties on top of existing ones,
    /// and writes the merged result back.
    pub(crate) fn set_edge_properties_impl(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        validate_property_keys(&properties)?;
        self.with_write_lock(
            graph,
            self.kp(key_edge_id_write_lock(graph, id.0)),
            WRITE_LOCK_TTL_MS,
            3,
            50,
            |conn| {
                self.require_graph_on(conn.as_mut(), graph)?;
                let edge = self
                    .read_edge(conn.as_mut(), graph, id.0)?
                    .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
                let mut merged = edge.properties;
                merged.extend(properties);
                let props_json = props_to_json(&merged)?;
                conn.renew_lock()?;
                redis::cmd("HSET")
                    .arg(self.kp(key_edge(graph, id.0)))
                    .arg("props")
                    .arg(&props_json)
                    .query::<()>(conn.as_mut())
                    .map_err(crate::backend_storage_error)
            },
        )
    }

    /// Replace all properties on an existing edge (REPLACE semantics).
    ///
    /// Unlike `set_edge_properties_impl`, this discards all existing properties
    /// and replaces them entirely with the provided map.
    pub(crate) fn replace_edge_properties_impl(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        validate_property_keys(&properties)?;
        self.with_write_lock(
            graph,
            self.kp(key_edge_id_write_lock(graph, id.0)),
            WRITE_LOCK_TTL_MS,
            3,
            50,
            |conn| {
                self.require_graph_on(conn.as_mut(), graph)?;
                if self.read_edge(conn.as_mut(), graph, id.0)?.is_none() {
                    return Err(StorageError::EdgeNotFound { id: id.0 });
                }
                let props_json = props_to_json(&properties)?;
                conn.renew_lock()?;
                redis::cmd("HSET")
                    .arg(self.kp(key_edge(graph, id.0)))
                    .arg("props")
                    .arg(&props_json)
                    .query::<()>(conn.as_mut())
                    .map_err(crate::backend_storage_error)
            },
        )
    }

    /// Remove a single property key from an edge.
    ///
    /// Reads the current edge, removes the specified key from its property map,
    /// and writes the remaining properties back.
    pub(crate) fn remove_edge_property_impl(
        &self,
        graph: &str,
        id: EdgeId,
        key: &str,
    ) -> Result<(), StorageError> {
        validate_property_key(key)?;
        self.with_write_lock(
            graph,
            self.kp(key_edge_id_write_lock(graph, id.0)),
            WRITE_LOCK_TTL_MS,
            3,
            50,
            |conn| {
                self.require_graph_on(conn.as_mut(), graph)?;
                let edge = self
                    .read_edge(conn.as_mut(), graph, id.0)?
                    .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
                let mut props = edge.properties;
                props.shift_remove(key);
                let props_json = props_to_json(&props)?;
                conn.renew_lock()?;
                redis::cmd("HSET")
                    .arg(self.kp(key_edge(graph, id.0)))
                    .arg("props")
                    .arg(&props_json)
                    .query::<()>(conn.as_mut())
                    .map_err(crate::backend_storage_error)
            },
        )
    }
}
