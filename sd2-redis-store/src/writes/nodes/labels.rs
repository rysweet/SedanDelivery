// writes/nodes/labels.rs -- Label add/remove operations on existing nodes.

use cypher_core::error::StorageError;
use cypher_core::storage::NodeId;

use crate::keys::{key_label, key_label_names, key_node, key_node_write_lock};
use crate::lock::WRITE_LOCK_TTL_MS;
use crate::RedisGraphStore;

use super::validate_labels;

impl RedisGraphStore {
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
        validate_labels(labels)?;
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
                let mut existing: std::collections::HashSet<String> =
                    node.labels.into_iter().collect();
                let mut added_labels = Vec::new();
                for label in labels {
                    if existing.insert(label.clone()) {
                        added_labels.push(label.as_str());
                    }
                }

                let new_labels: Vec<String> = existing.into_iter().collect();
                let labels_json = serde_json::to_string(&new_labels)
                    .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;

                let mut pipe = conn.pipeline();
                let id_str = id.0.to_string();
                for label in &added_labels {
                    pipe.cmd("SADD")
                        .arg(self.kp(key_label(graph, label)))
                        .arg(&id_str)
                        .ignore();
                }
                if !added_labels.is_empty() {
                    let cmd = pipe.cmd("SADD").arg(self.kp(key_label_names(graph)));
                    for label in &added_labels {
                        cmd.arg(label);
                    }
                    cmd.ignore();
                }
                pipe.cmd("HSET")
                    .arg(self.kp(key_node(graph, id.0)))
                    .arg("labels")
                    .arg(&labels_json)
                    .ignore();
                conn.renew_lock()?;
                pipe.query::<()>(conn)
                    .map_err(crate::backend_storage_error)?;
                let added_labels_owned: Vec<String> = added_labels
                    .into_iter()
                    .map(|label| label.to_string())
                    .collect();
                if !added_labels_owned.is_empty() {
                    self.index_node_properties(
                        conn,
                        graph,
                        id.0,
                        &added_labels_owned,
                        &node.properties,
                    )?;
                }
                Ok(())
            },
        )
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
        validate_labels(labels)?;
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
                let to_remove: std::collections::HashSet<&String> = labels.iter().collect();
                let removed_labels: Vec<String> = node
                    .labels
                    .iter()
                    .filter(|label| to_remove.contains(label))
                    .cloned()
                    .collect();
                if removed_labels.is_empty() {
                    return Ok(());
                }
                self.deindex_node_properties(conn, graph, id.0, &removed_labels, &node.properties)?;
                let id_str = id.0.to_string();
                let mut pipe = conn.pipeline();
                for label in &to_remove {
                    pipe.cmd("SREM")
                        .arg(self.kp(key_label(graph, label)))
                        .arg(&id_str)
                        .ignore();
                }
                let new_labels: Vec<String> = node
                    .labels
                    .into_iter()
                    .filter(|l| !to_remove.contains(l))
                    .collect();
                let labels_json = serde_json::to_string(&new_labels)
                    .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;
                pipe.cmd("HSET")
                    .arg(self.kp(key_node(graph, id.0)))
                    .arg("labels")
                    .arg(&labels_json)
                    .ignore();
                conn.renew_lock()?;
                pipe.query::<()>(conn)
                    .map_err(crate::backend_storage_error)?;
                Ok(())
            },
        )
    }
}
