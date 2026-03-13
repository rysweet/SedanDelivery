// writes/nodes.rs -- Node write operations for RedisGraphStore.

use std::collections::HashMap;

use gql_core::error::StorageError;
use gql_core::serde_value::props_to_json;
use gql_core::storage::NodeId;
use gql_core::types::Value;

use crate::keys::{
    key_in, key_label, key_node, key_node_seq, key_nodes_set, key_out, key_graph_lock,
};
use crate::lock::{self, WRITE_LOCK_TTL_MS};
use crate::RedisGraphStore;

use super::helpers::{clean_fanout_props, extract_injected_id};

impl RedisGraphStore {
    /// Create a single node in the given graph.
    ///
    /// Allocates a new node ID via Redis INCRBY on the node sequence key,
    /// unless the special `_sd2_id` property is present (injected by FanoutStore
    /// for cross-cluster ID stability). Stores the node hash, updates label
    /// indexes, and the all-nodes set atomically via a Redis pipeline.
    ///
    /// Property indexing is performed after the main pipeline completes.
    pub(crate) fn create_node_impl(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: HashMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        self.require_graph(graph)?;
        let mut conn = self.conn()?;

        // If FanoutStore injected _sd2_id, use it instead of INCR.
        let id = match extract_injected_id(&properties, "_sd2_id")? {
            Some(id) => id,
            None => {
                let raw_id: i64 = redis::cmd("INCRBY")
                    .arg(self.kp(key_node_seq(graph)))
                    .arg(1)
                    .query(conn.as_mut())
                    .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
                if raw_id <= 0 {
                    return Err(StorageError::IdOverflow {
                        graph: graph.to_string(),
                    });
                }
                raw_id as u64
            }
        };

        // Strip internal FanoutStore keys before storing.
        let clean_props = clean_fanout_props(properties);

        let node_key = self.kp(key_node(graph, id));
        let labels_json = serde_json::to_string(&labels)
            .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;
        let props_json = props_to_json(&clean_props)?;
        // Store node hash, label indexes, and all-nodes set atomically.
        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("HSET")
            .arg(&node_key)
            .arg("labels")
            .arg(&labels_json)
            .arg("props")
            .arg(&props_json)
            .ignore();
        for label in &labels {
            pipe.cmd("SADD")
                .arg(self.kp(key_label(graph, label)))
                .arg(id.to_string())
                .ignore();
        }
        pipe.cmd("SADD")
            .arg(self.kp(key_nodes_set(graph)))
            .arg(id.to_string())
            .ignore();
        pipe.query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Maintain property indexes.
        self.index_node_properties(conn.as_mut(), graph, id, &labels, &clean_props)?;
        Ok(NodeId(id))
    }

    /// Merge additional properties into an existing node (SET semantics).
    ///
    /// Reads the current node, deindexes old property values, merges new
    /// properties on top, writes the merged set back, and reindexes.
    pub(crate) fn set_node_properties_impl(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let lock_key = self.kp(key_graph_lock(graph));
        let token = lock::acquire_blocking(conn.as_mut(), &lock_key, WRITE_LOCK_TTL_MS, 3, 50)?;
        let result = (|| -> Result<(), StorageError> {
            let node = self
                .read_node(conn.as_mut(), graph, id.0)?
                .ok_or(StorageError::NodeNotFound { id: id.0 })?;
            // Deindex old property values before merging.
            self.deindex_node_properties(conn.as_mut(), graph, id.0, &node.labels, &node.properties)?;
            let mut merged = node.properties;
            merged.extend(properties);
            let props_json = props_to_json(&merged)?;
            redis::cmd("HSET")
                .arg(self.kp(key_node(graph, id.0)))
                .arg("props")
                .arg(&props_json)
                .query::<()>(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            // Reindex with new values.
            self.index_node_properties(conn.as_mut(), graph, id.0, &node.labels, &merged)?;
            Ok(())
        })();
        let _ = lock::release(conn.as_mut(), &lock_key, &token);
        result
    }

    /// Replace all properties on an existing node (REPLACE semantics).
    ///
    /// Unlike `set_node_properties_impl`, this discards all existing properties
    /// and replaces them entirely with the provided map.
    pub(crate) fn replace_node_properties_impl(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let lock_key = self.kp(key_graph_lock(graph));
        let token = lock::acquire_blocking(conn.as_mut(), &lock_key, WRITE_LOCK_TTL_MS, 3, 50)?;
        let result = (|| -> Result<(), StorageError> {
            let node = self
                .read_node(conn.as_mut(), graph, id.0)?
                .ok_or(StorageError::NodeNotFound { id: id.0 })?;
            // Deindex old values.
            self.deindex_node_properties(conn.as_mut(), graph, id.0, &node.labels, &node.properties)?;
            let props_json = props_to_json(&properties)?;
            redis::cmd("HSET")
                .arg(self.kp(key_node(graph, id.0)))
                .arg("props")
                .arg(&props_json)
                .query::<()>(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            // Index new values.
            self.index_node_properties(conn.as_mut(), graph, id.0, &node.labels, &properties)?;
            Ok(())
        })();
        let _ = lock::release(conn.as_mut(), &lock_key, &token);
        result
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
        let mut conn = self.conn()?;
        let lock_key = self.kp(key_graph_lock(graph));
        let token = lock::acquire_blocking(conn.as_mut(), &lock_key, WRITE_LOCK_TTL_MS, 3, 50)?;
        let result = (|| -> Result<(), StorageError> {
            let node = self
                .read_node(conn.as_mut(), graph, id.0)?
                .ok_or(StorageError::NodeNotFound { id: id.0 })?;
            // Deindex old values before removal.
            self.deindex_node_properties(conn.as_mut(), graph, id.0, &node.labels, &node.properties)?;
            let mut props = node.properties;
            props.remove(key);
            let props_json = props_to_json(&props)?;
            redis::cmd("HSET")
                .arg(self.kp(key_node(graph, id.0)))
                .arg("props")
                .arg(&props_json)
                .query::<()>(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            // Reindex with remaining values.
            self.index_node_properties(conn.as_mut(), graph, id.0, &node.labels, &props)?;
            Ok(())
        })();
        let _ = lock::release(conn.as_mut(), &lock_key, &token);
        result
    }

    /// Add labels to an existing node without removing any existing labels.
    ///
    /// For each genuinely new label, adds the node ID to that label's index set.
    /// Then writes the updated label list back to the node hash.
    pub(crate) fn add_node_labels_impl(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let lock_key = self.kp(key_graph_lock(graph));
        let token = lock::acquire_blocking(conn.as_mut(), &lock_key, WRITE_LOCK_TTL_MS, 3, 50)?;
        let result = (|| -> Result<(), StorageError> {
            let node = self
                .read_node(conn.as_mut(), graph, id.0)?
                .ok_or(StorageError::NodeNotFound { id: id.0 })?;
            let mut existing: std::collections::HashSet<String> = node.labels.into_iter().collect();
            let id_str = id.0.to_string();
            let mut pipe = redis::pipe();
            let mut has_new = false;
            for label in labels {
                if existing.insert(label.clone()) {
                    pipe.cmd("SADD")
                        .arg(self.kp(key_label(graph, label)))
                        .arg(&id_str)
                        .ignore();
                    has_new = true;
                }
            }
            if has_new {
                pipe.query::<()>(conn.as_mut())
                    .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            }
            let new_labels: Vec<String> = existing.into_iter().collect();
            let labels_json = serde_json::to_string(&new_labels)
                .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;
            redis::cmd("HSET")
                .arg(self.kp(key_node(graph, id.0)))
                .arg("labels")
                .arg(&labels_json)
                .query::<()>(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })
        })();
        let _ = lock::release(conn.as_mut(), &lock_key, &token);
        result
    }

    /// Remove labels from an existing node.
    ///
    /// For each label being removed, removes the node ID from that label's
    /// index set. Then writes the filtered label list back to the node hash.
    pub(crate) fn remove_node_labels_impl(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let lock_key = self.kp(key_graph_lock(graph));
        let token = lock::acquire_blocking(conn.as_mut(), &lock_key, WRITE_LOCK_TTL_MS, 3, 50)?;
        let result = (|| -> Result<(), StorageError> {
            let node = self
                .read_node(conn.as_mut(), graph, id.0)?
                .ok_or(StorageError::NodeNotFound { id: id.0 })?;
            let to_remove: std::collections::HashSet<&String> = labels.iter().collect();
            if !to_remove.is_empty() {
                let id_str = id.0.to_string();
                let mut pipe = redis::pipe();
                for label in &to_remove {
                    pipe.cmd("SREM")
                        .arg(self.kp(key_label(graph, label)))
                        .arg(&id_str)
                        .ignore();
                }
                pipe.query::<()>(conn.as_mut())
                    .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            }
            let new_labels: Vec<String> = node
                .labels
                .into_iter()
                .filter(|l| !to_remove.contains(l))
                .collect();
            let labels_json = serde_json::to_string(&new_labels)
                .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;
            redis::cmd("HSET")
                .arg(self.kp(key_node(graph, id.0)))
                .arg("labels")
                .arg(&labels_json)
                .query::<()>(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })
        })();
        let _ = lock::release(conn.as_mut(), &lock_key, &token);
        result
    }

    /// Delete a node and optionally detach (delete) all its edges first.
    ///
    /// When `detach` is false, returns an error if the node has any edges.
    /// When `detach` is true, deletes all adjacent edges (both incoming and
    /// outgoing) before removing the node itself.
    ///
    /// Node deletion removes: label index entries, the all-nodes set entry,
    /// adjacency keys, property indexes, and the node hash.
    pub(crate) fn delete_node_impl(
        &self,
        graph: &str,
        id: NodeId,
        detach: bool,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let lock_key = self.kp(key_graph_lock(graph));
        let token = lock::acquire_blocking(conn.as_mut(), &lock_key, WRITE_LOCK_TTL_MS, 3, 50)?;
        let result = (|| -> Result<(), StorageError> {
            let node = self
                .read_node(conn.as_mut(), graph, id.0)?
                .ok_or(StorageError::NodeNotFound { id: id.0 })?;
            // Check for relationships if not detaching.
            if !detach {
                let out: Vec<String> = redis::cmd("SMEMBERS")
                    .arg(self.kp(key_out(graph, id.0)))
                    .query(conn.as_mut())
                    .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
                let in_: Vec<String> = redis::cmd("SMEMBERS")
                    .arg(self.kp(key_in(graph, id.0)))
                    .query(conn.as_mut())
                    .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
                if !out.is_empty() || !in_.is_empty() {
                    return Err(StorageError::Backend {
                        msg: format!("node {} has relationships; use DETACH DELETE", id.0),
                    });
                }
            }
            if detach {
                // Delete all adjacent edges first, keeping the lock held throughout
                // to prevent new edges from arriving during the operation.
                let out_members: Vec<String> = redis::cmd("SMEMBERS")
                    .arg(self.kp(key_out(graph, id.0)))
                    .query(conn.as_mut())
                    .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
                let in_members: Vec<String> = redis::cmd("SMEMBERS")
                    .arg(self.kp(key_in(graph, id.0)))
                    .query(conn.as_mut())
                    .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
                let mut edge_ids: Vec<gql_core::storage::EdgeId> = Vec::new();
                for m in out_members.iter().chain(in_members.iter()) {
                    if let Some(adj) = crate::keys::decode_adj(m) {
                        edge_ids.push(adj.edge_id);
                    }
                }
                // Deduplicate and delete each edge while holding the lock.
                edge_ids.sort_unstable();
                edge_ids.dedup();
                for eid in edge_ids {
                    self.delete_edge_inner(conn.as_mut(), graph, eid)?;
                }
            }
            // Remove property indexes (requires reads, so must stay outside transaction).
            self.deindex_node_properties(conn.as_mut(), graph, id.0, &node.labels, &node.properties)?;
            // Remove label indexes, all-nodes set, adjacency keys, and node hash atomically.
            let id_str = id.0.to_string();
            let mut pipe = redis::pipe();
            for label in &node.labels {
                pipe.cmd("SREM")
                    .arg(self.kp(key_label(graph, label)))
                    .arg(&id_str)
                    .ignore();
            }
            pipe.cmd("SREM")
                .arg(self.kp(key_nodes_set(graph)))
                .arg(&id_str)
                .ignore();
            pipe.cmd("DEL").arg(self.kp(key_out(graph, id.0))).ignore();
            pipe.cmd("DEL").arg(self.kp(key_in(graph, id.0))).ignore();
            pipe.cmd("DEL").arg(self.kp(key_node(graph, id.0))).ignore();
            pipe.query::<()>(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            Ok(())
        })();
        let _ = lock::release(conn.as_mut(), &lock_key, &token);
        result
    }

    /// Batch-create multiple nodes in a single Redis pipeline round-trip.
    ///
    /// For each node: INCRBY to allocate IDs, HSET to store node data,
    /// SADD to update label indexes and the all-nodes set.
    /// Property indexing is performed in a second pipeline pass.
    ///
    /// Returns the newly created `NodeId`s in the same order as the input.
    pub fn batch_create_nodes(
        &self,
        graph: &str,
        nodes: Vec<(Vec<String>, HashMap<String, Value>)>,
    ) -> Result<Vec<NodeId>, StorageError> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }
        self.require_graph(graph)?;
        let mut conn = self.conn()?;

        let count = i64::try_from(nodes.len()).map_err(|_| StorageError::Backend {
            msg: format!(
                "batch size {} exceeds i64::MAX, cannot allocate IDs",
                nodes.len()
            ),
        })?;

        // Allocate a contiguous block of IDs with a single INCRBY.
        let end_id: i64 = redis::cmd("INCRBY")
            .arg(self.kp(key_node_seq(graph)))
            .arg(count)
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        if end_id <= 0 || end_id < count {
            return Err(StorageError::IdOverflow {
                graph: graph.to_string(),
            });
        }
        let start_id = (end_id - count + 1) as u64;

        // Pre-serialize all nodes and build the pipeline.
        let mut pipe = redis::pipe();
        let mut node_meta: Vec<(u64, Vec<String>, HashMap<String, Value>)> =
            Vec::with_capacity(nodes.len());

        for (i, (labels, properties)) in nodes.into_iter().enumerate() {
            let id = start_id + i as u64;

            // Strip internal FanoutStore keys before storing.
            let clean_props = clean_fanout_props(properties);

            let node_key = self.kp(key_node(graph, id));
            let labels_json = serde_json::to_string(&labels)
                .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;
            let props_json = props_to_json(&clean_props)?;

            // HSET node hash.
            pipe.cmd("HSET")
                .arg(&node_key)
                .arg("labels")
                .arg(&labels_json)
                .arg("props")
                .arg(&props_json)
                .ignore();

            // SADD to label indexes.
            for label in &labels {
                pipe.cmd("SADD")
                    .arg(self.kp(key_label(graph, label)))
                    .arg(id.to_string())
                    .ignore();
            }

            // SADD to all-nodes set.
            pipe.cmd("SADD")
                .arg(self.kp(key_nodes_set(graph)))
                .arg(id.to_string())
                .ignore();

            node_meta.push((id, labels, clean_props));
        }

        // Execute the main pipeline in one round-trip.
        pipe.query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;

        // Property indexing pass (uses its own pipeline internally).
        for (id, labels, props) in &node_meta {
            self.index_node_properties(conn.as_mut(), graph, *id, labels, props)?;
        }

        Ok(node_meta.iter().map(|(id, _, _)| NodeId(*id)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writes::helpers::{clean_fanout_props, extract_injected_id};

    // ---- Pure logic tests for extract_injected_id ---------------------------

    #[test]
    fn extract_injected_id_present_positive() {
        let mut props = HashMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(42));
        let result = extract_injected_id(&props, "_sd2_id").unwrap();
        assert_eq!(result, Some(42));
    }

    #[test]
    fn extract_injected_id_present_zero() {
        let mut props = HashMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(0));
        let result = extract_injected_id(&props, "_sd2_id").unwrap();
        assert_eq!(result, Some(0));
    }

    #[test]
    fn extract_injected_id_present_negative_fails() {
        let mut props = HashMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(-5));
        let result = extract_injected_id(&props, "_sd2_id");
        assert!(result.is_err());
    }

    #[test]
    fn extract_injected_id_absent() {
        let props = HashMap::new();
        let result = extract_injected_id(&props, "_sd2_id").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn extract_injected_id_wrong_type_ignored() {
        let mut props = HashMap::new();
        props.insert("_sd2_id".to_string(), Value::Str("not_an_int".into()));
        // Non-Int values are not matched, so returns None.
        let result = extract_injected_id(&props, "_sd2_id").unwrap();
        assert_eq!(result, None);
    }

    // ---- Pure logic tests for clean_fanout_props ----------------------------

    #[test]
    fn clean_fanout_props_removes_internal_keys() {
        let mut props = HashMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(1));
        props.insert("_sd2_eid".to_string(), Value::Int(2));
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let cleaned = clean_fanout_props(props);
        assert!(!cleaned.contains_key("_sd2_id"));
        assert!(!cleaned.contains_key("_sd2_eid"));
        assert_eq!(cleaned.get("name"), Some(&Value::Str("Alice".into())));
    }

    #[test]
    fn clean_fanout_props_noop_when_no_internal_keys() {
        let mut props = HashMap::new();
        props.insert("age".to_string(), Value::Int(30));
        let cleaned = clean_fanout_props(props);
        assert_eq!(cleaned.len(), 1);
        assert_eq!(cleaned.get("age"), Some(&Value::Int(30)));
    }

    #[test]
    fn clean_fanout_props_empty_map() {
        let props = HashMap::new();
        let cleaned = clean_fanout_props(props);
        assert!(cleaned.is_empty());
    }
}
