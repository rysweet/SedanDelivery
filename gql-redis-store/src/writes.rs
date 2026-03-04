// writes.rs — All GraphStore write method implementations for RedisGraphStore.

use std::collections::HashMap;

use redis::ConnectionLike;

use gql_core::error::StorageError;
use gql_core::serde_value::props_to_json;
use gql_core::storage::{validate_graph_name, EdgeId, NodeId};
use gql_core::types::Value;

use crate::keys::{
    decode_adj, encode_adj, key_edge, key_edge_seq, key_edges_set, key_in, key_label, key_node,
    key_node_seq, key_nodes_set, key_out, key_reltype,
};
use crate::RedisGraphStore;

impl RedisGraphStore {
    /// Delete an edge using an already-locked connection. This avoids dropping
    /// and re-acquiring the lock when called from `delete_node(detach=true)`.
    pub(crate) fn delete_edge_inner(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        id: EdgeId,
    ) -> Result<(), StorageError> {
        let edge = self
            .read_edge(conn, graph, id.0)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        // Remove from rel-type index.
        redis::cmd("SREM")
            .arg(self.kp(key_reltype(graph, &edge.rel_type)))
            .arg(id.0.to_string())
            .query::<()>(conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Remove from all-edges set.
        redis::cmd("SREM")
            .arg(self.kp(key_edges_set(graph)))
            .arg(id.0.to_string())
            .query::<()>(conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Remove from adjacency sets.
        let out_member = encode_adj(id, edge.dst, &edge.rel_type);
        let in_member = encode_adj(id, edge.src, &edge.rel_type);
        redis::cmd("SREM")
            .arg(self.kp(key_out(graph, edge.src.0)))
            .arg(out_member)
            .query::<()>(conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        redis::cmd("SREM")
            .arg(self.kp(key_in(graph, edge.dst.0)))
            .arg(in_member)
            .query::<()>(conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Delete edge hash.
        redis::cmd("DEL")
            .arg(self.kp(key_edge(graph, id.0)))
            .query::<()>(conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(())
    }

    // ── GraphStore trait method implementations for writes ───────────────────

    pub(crate) fn create_node_impl(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: HashMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        self.require_graph(graph)?;
        let mut conn = self.conn()?;

        // If FanoutStore injected _sd2_id, use it instead of INCR.
        let id = if let Some(Value::Int(i)) = properties.get("_sd2_id") {
            if *i < 0 {
                return Err(StorageError::Backend {
                    msg: format!("_sd2_id must be non-negative, got {i}"),
                });
            }
            *i as u64
        } else {
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
        };

        // Strip internal FanoutStore keys before storing.
        let mut clean_props = properties;
        clean_props.remove("_sd2_id");
        clean_props.remove("_sd2_eid");

        let node_key = self.kp(key_node(graph, id));
        let labels_json = serde_json::to_string(&labels)
            .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;
        let props_json = props_to_json(&clean_props)?;
        // Store node hash.
        redis::cmd("HSET")
            .arg(&node_key)
            .arg("labels")
            .arg(&labels_json)
            .arg("props")
            .arg(&props_json)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Update label indexes.
        for label in &labels {
            redis::cmd("SADD")
                .arg(self.kp(key_label(graph, label)))
                .arg(id.to_string())
                .query::<()>(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        }
        // Update all-nodes set.
        redis::cmd("SADD")
            .arg(self.kp(key_nodes_set(graph)))
            .arg(id.to_string())
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Maintain property indexes.
        self.index_node_properties(conn.as_mut(), graph, id, &labels, &clean_props)?;
        Ok(NodeId(id))
    }

    pub(crate) fn create_edge_impl(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: HashMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        self.require_graph(graph)?;
        let mut conn = self.conn()?;
        // Verify src and dst exist.
        if self.read_node(conn.as_mut(), graph, src.0)?.is_none() {
            return Err(StorageError::NodeNotFound { id: src.0 });
        }
        if self.read_node(conn.as_mut(), graph, dst.0)?.is_none() {
            return Err(StorageError::NodeNotFound { id: dst.0 });
        }

        // If FanoutStore injected _sd2_eid, use it instead of INCR.
        let id = if let Some(Value::Int(i)) = properties.get("_sd2_eid") {
            if *i < 0 {
                return Err(StorageError::Backend {
                    msg: format!("_sd2_eid must be non-negative, got {i}"),
                });
            }
            *i as u64
        } else {
            let raw_id: i64 = redis::cmd("INCRBY")
                .arg(self.kp(key_edge_seq(graph)))
                .arg(1)
                .query(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            if raw_id <= 0 {
                return Err(StorageError::IdOverflow {
                    graph: graph.to_string(),
                });
            }
            raw_id as u64
        };

        // Strip internal FanoutStore keys before storing.
        let mut clean_props = properties;
        clean_props.remove("_sd2_id");
        clean_props.remove("_sd2_eid");

        let edge_key = self.kp(key_edge(graph, id));
        let props_json = props_to_json(&clean_props)?;
        redis::cmd("HSET")
            .arg(&edge_key)
            .arg("src")
            .arg(src.0.to_string())
            .arg("dst")
            .arg(dst.0.to_string())
            .arg("rel_type")
            .arg(&rel_type)
            .arg("props")
            .arg(&props_json)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Update rel-type index.
        redis::cmd("SADD")
            .arg(self.kp(key_reltype(graph, &rel_type)))
            .arg(id.to_string())
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Update adjacency sets.
        let out_member = encode_adj(EdgeId(id), dst, &rel_type);
        let in_member = encode_adj(EdgeId(id), src, &rel_type);
        redis::cmd("SADD")
            .arg(self.kp(key_out(graph, src.0)))
            .arg(out_member)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        redis::cmd("SADD")
            .arg(self.kp(key_in(graph, dst.0)))
            .arg(in_member)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Update all-edges set.
        redis::cmd("SADD")
            .arg(self.kp(key_edges_set(graph)))
            .arg(id.to_string())
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(EdgeId(id))
    }

    pub(crate) fn set_node_properties_impl(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
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
    }

    pub(crate) fn replace_node_properties_impl(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
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
    }

    pub(crate) fn set_edge_properties_impl(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let edge = self
            .read_edge(conn.as_mut(), graph, id.0)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        let mut merged = edge.properties;
        merged.extend(properties);
        let props_json = props_to_json(&merged)?;
        redis::cmd("HSET")
            .arg(self.kp(key_edge(graph, id.0)))
            .arg("props")
            .arg(&props_json)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    pub(crate) fn replace_edge_properties_impl(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        if self.read_edge(conn.as_mut(), graph, id.0)?.is_none() {
            return Err(StorageError::EdgeNotFound { id: id.0 });
        }
        let props_json = props_to_json(&properties)?;
        redis::cmd("HSET")
            .arg(self.kp(key_edge(graph, id.0)))
            .arg("props")
            .arg(&props_json)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    pub(crate) fn remove_node_property_impl(
        &self,
        graph: &str,
        id: NodeId,
        key: &str,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
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
    }

    pub(crate) fn remove_edge_property_impl(
        &self,
        graph: &str,
        id: EdgeId,
        key: &str,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let edge = self
            .read_edge(conn.as_mut(), graph, id.0)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        let mut props = edge.properties;
        props.remove(key);
        let props_json = props_to_json(&props)?;
        redis::cmd("HSET")
            .arg(self.kp(key_edge(graph, id.0)))
            .arg("props")
            .arg(&props_json)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    pub(crate) fn add_node_labels_impl(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let node = self
            .read_node(conn.as_mut(), graph, id.0)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        let mut existing: std::collections::HashSet<String> = node.labels.into_iter().collect();
        for label in labels {
            if existing.insert(label.clone()) {
                // New label: update index.
                redis::cmd("SADD")
                    .arg(self.kp(key_label(graph, label)))
                    .arg(id.0.to_string())
                    .query::<()>(conn.as_mut())
                    .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            }
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
    }

    pub(crate) fn remove_node_labels_impl(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let node = self
            .read_node(conn.as_mut(), graph, id.0)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        let to_remove: std::collections::HashSet<&String> = labels.iter().collect();
        for label in &to_remove {
            redis::cmd("SREM")
                .arg(self.kp(key_label(graph, label)))
                .arg(id.0.to_string())
                .query::<()>(conn.as_mut())
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
    }

    pub(crate) fn delete_node_impl(
        &self,
        graph: &str,
        id: NodeId,
        detach: bool,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
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
            let mut edge_ids: Vec<EdgeId> = Vec::new();
            for m in out_members.iter().chain(in_members.iter()) {
                if let Some(adj) = decode_adj(m) {
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
        // Remove property indexes.
        self.deindex_node_properties(conn.as_mut(), graph, id.0, &node.labels, &node.properties)?;
        // Remove label indexes.
        for label in &node.labels {
            redis::cmd("SREM")
                .arg(self.kp(key_label(graph, label)))
                .arg(id.0.to_string())
                .query::<()>(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        }
        // Remove from all-nodes set.
        redis::cmd("SREM")
            .arg(self.kp(key_nodes_set(graph)))
            .arg(id.0.to_string())
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Delete adjacency keys.
        redis::cmd("DEL")
            .arg(self.kp(key_out(graph, id.0)))
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        redis::cmd("DEL")
            .arg(self.kp(key_in(graph, id.0)))
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Delete node hash.
        redis::cmd("DEL")
            .arg(self.kp(key_node(graph, id.0)))
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(())
    }

    pub(crate) fn delete_edge_impl(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        self.delete_edge_inner(conn.as_mut(), graph, id)
    }

    // ── Graph administration writes ─────────────────────────────────────────

    pub(crate) fn create_graph_impl(&self, name: &str) -> Result<(), StorageError> {
        validate_graph_name(name)?;
        let mut conn = self.conn()?;
        let added: i64 = redis::cmd("SADD")
            .arg(self.key_graphs())
            .arg(name)
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        if added == 0 {
            return Err(StorageError::GraphAlreadyExists {
                name: name.to_string(),
            });
        }
        Ok(())
    }

    pub(crate) fn delete_graph_impl(&self, name: &str) -> Result<(), StorageError> {
        if !self.graph_exists_impl(name)? {
            return Err(StorageError::GraphNotFound {
                name: name.to_string(),
            });
        }
        let mut conn = self.conn()?;
        // Remove from registry.
        redis::cmd("SREM")
            .arg(self.key_graphs())
            .arg(name)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Delete all keys with pattern G:{name}:* using SCAN.
        let pattern = format!("{}G:{{{name}}}:*", self.key_prefix);
        let mut cursor: u64 = 0;
        loop {
            let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(100u64)
                .query(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            if !keys.is_empty() {
                let mut del_cmd = redis::cmd("DEL");
                for k in &keys {
                    del_cmd.arg(k);
                }
                del_cmd
                    .query::<()>(conn.as_mut())
                    .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            }
            cursor = next_cursor;
            if cursor == 0 {
                break;
            }
        }
        Ok(())
    }
}
