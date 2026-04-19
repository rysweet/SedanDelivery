//! Node-level operations for `ModuleRedisStore`.
//!
//! These are inherent methods invoked by the `GraphStore` trait impl in
//! `mod.rs`. Each method mirrors the corresponding trait method's signature.

use indexmap::IndexMap;

use cypher_core::error::StorageError;
use cypher_core::serde_value::props_to_json;
use cypher_core::storage::{NodeData, NodeId, RangeQuery};
use cypher_core::types::Value;

use super::keys::{
    key_in, key_label, key_label_names, key_node, key_node_seq, key_nodes_set, key_out,
    key_propidx, value_to_index_key,
};
use super::ModuleRedisStore;

impl ModuleRedisStore {
    pub(super) fn impl_get_node(
        &self,
        graph: &str,
        id: NodeId,
    ) -> Result<Option<NodeData>, StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_node(graph, id.0));
            let result = self.call_cmd(ctx, "HMGET", &[&key, "labels", "props"])?;
            let fields = Self::extract_hmget_fields(&result);
            self.parse_node_from_hmget(id, &fields)
        })
    }

    pub(super) fn impl_find_nodes_by_label(
        &self,
        graph: &str,
        label: &str,
    ) -> Result<Vec<NodeId>, StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_label(graph, label));
            let result = self.call_cmd(ctx, "SMEMBERS", &[&key])?;
            Self::extract_string_array(&result)
                .into_iter()
                .map(|s| {
                    s.parse::<u64>()
                        .map(NodeId)
                        .map_err(|_| StorageError::Serialization {
                            msg: format!("bad node id in label index: {s}"),
                        })
                })
                .collect()
        })
    }

    pub(super) fn impl_find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        // Use property index for exact-match queries when an index exists.
        if let RangeQuery::Eq(val) = &range {
            let indexes = self.active_indexes(graph)?;
            if indexes.contains(&format!("{label}\x00{property}")) {
                let vkey = value_to_index_key(val);
                let idx_key = self.kp(key_propidx(graph, label, property, &vkey));
                return self.with_ctx(|ctx| {
                    let result = self.call_cmd(ctx, "SMEMBERS", &[&idx_key])?;
                    Self::extract_string_array(&result)
                        .into_iter()
                        .map(|s| {
                            s.parse::<u64>()
                                .map(NodeId)
                                .map_err(|_| StorageError::Serialization {
                                    msg: format!("bad node id in propidx: {s}"),
                                })
                        })
                        .collect()
                });
            }
        }

        // No silent fallback: refuse to scan all nodes.
        Err(StorageError::NoPropertyIndex {
            label: label.to_string(),
            property: property.to_string(),
        })
    }

    pub(super) fn impl_all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_nodes_set(graph));
            let result = self.call_cmd(ctx, "SMEMBERS", &[&key])?;
            Self::extract_string_array(&result)
                .into_iter()
                .map(|s| {
                    s.parse::<u64>()
                        .map(NodeId)
                        .map_err(|_| StorageError::Serialization {
                            msg: format!("bad node id in nodes set: {s}"),
                        })
                })
                .collect()
        })
    }

    pub(super) fn impl_create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        self.with_ctx(|ctx| {
            // Allocate ID.
            let seq_key = self.kp(key_node_seq(graph));
            let result = self.call_cmd(ctx, "INCRBY", &[&seq_key, "1"])?;
            let raw_id = Self::expect_integer(&result, "INCRBY")?;
            if raw_id <= 0 {
                return Err(StorageError::IdOverflow {
                    graph: graph.to_string(),
                });
            }
            let id = raw_id as u64;

            // Serialize.
            let labels_json = serde_json::to_string(&labels)
                .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;
            let props_json = props_to_json(&properties)?;

            // HSET node hash.
            let node_key = self.kp(key_node(graph, id));
            self.call_cmd(
                ctx,
                "HSET",
                &[&node_key, "labels", &labels_json, "props", &props_json],
            )?;

            // SADD to all-nodes set.
            let nodes_set_key = self.kp(key_nodes_set(graph));
            let id_str = id.to_string();
            self.call_cmd(ctx, "SADD", &[&nodes_set_key, &id_str])?;

            // SADD to each label index.
            for label in &labels {
                let label_key = self.kp(key_label(graph, label));
                self.call_cmd(ctx, "SADD", &[&label_key, &id_str])?;
            }

            // SADD label names to registry.
            if !labels.is_empty() {
                let registry_key = self.kp(key_label_names(graph));
                // ctx.call expects &[&str], build dynamically.
                let mut args: Vec<&str> = vec![&registry_key];
                for label in &labels {
                    args.push(label.as_str());
                }
                self.call_cmd(ctx, "SADD", &args)?;
            }

            Ok(NodeId(id))
        })
    }

    pub(super) fn impl_set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        let node = self
            .impl_get_node(graph, id)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        let mut merged = node.properties;
        merged.extend(properties);
        let props_json = props_to_json(&merged)?;
        self.with_ctx(|ctx| {
            let node_key = self.kp(key_node(graph, id.0));
            self.call_cmd(ctx, "HSET", &[&node_key, "props", &props_json])?;
            Ok(())
        })
    }

    pub(super) fn impl_replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        // Verify node exists.
        if self.impl_get_node(graph, id)?.is_none() {
            return Err(StorageError::NodeNotFound { id: id.0 });
        }
        let props_json = props_to_json(&properties)?;
        self.with_ctx(|ctx| {
            let node_key = self.kp(key_node(graph, id.0));
            self.call_cmd(ctx, "HSET", &[&node_key, "props", &props_json])?;
            Ok(())
        })
    }

    pub(super) fn impl_remove_node_property(
        &self,
        graph: &str,
        id: NodeId,
        key: &str,
    ) -> Result<(), StorageError> {
        let node = self
            .impl_get_node(graph, id)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        let mut props = node.properties;
        props.shift_remove(key);
        let props_json = props_to_json(&props)?;
        self.with_ctx(|ctx| {
            let node_key = self.kp(key_node(graph, id.0));
            self.call_cmd(ctx, "HSET", &[&node_key, "props", &props_json])?;
            Ok(())
        })
    }

    pub(super) fn impl_add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let node = self
            .impl_get_node(graph, id)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;

        let existing: std::collections::HashSet<&String> = node.labels.iter().collect();
        let new_labels: Vec<&String> = labels.iter().filter(|l| !existing.contains(l)).collect();
        if new_labels.is_empty() {
            return Ok(());
        }

        // Merge label lists.
        let mut all_labels = node.labels.clone();
        for label in &new_labels {
            all_labels.push((*label).clone());
        }
        let labels_json = serde_json::to_string(&all_labels)
            .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;

        self.with_ctx(|ctx| {
            // Update node hash.
            let node_key = self.kp(key_node(graph, id.0));
            self.call_cmd(ctx, "HSET", &[&node_key, "labels", &labels_json])?;

            // SADD to label indexes.
            let id_str = id.0.to_string();
            for label in &new_labels {
                let label_key = self.kp(key_label(graph, label));
                self.call_cmd(ctx, "SADD", &[&label_key, &id_str])?;
            }

            // Update label registry.
            let registry_key = self.kp(key_label_names(graph));
            let label_strs: Vec<&str> = new_labels.iter().map(|l| l.as_str()).collect();
            let mut args: Vec<&str> = vec![&registry_key];
            args.extend(label_strs);
            self.call_cmd(ctx, "SADD", &args)?;

            Ok(())
        })
    }

    pub(super) fn impl_remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let node = self
            .impl_get_node(graph, id)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;

        let to_remove: std::collections::HashSet<&String> = labels.iter().collect();
        let removed: Vec<String> = node
            .labels
            .iter()
            .filter(|l| to_remove.contains(l))
            .cloned()
            .collect();
        if removed.is_empty() {
            return Ok(());
        }

        let new_labels: Vec<String> = node
            .labels
            .into_iter()
            .filter(|l| !to_remove.contains(l))
            .collect();
        let labels_json = serde_json::to_string(&new_labels)
            .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;

        self.with_ctx(|ctx| {
            let node_key = self.kp(key_node(graph, id.0));
            self.call_cmd(ctx, "HSET", &[&node_key, "labels", &labels_json])?;

            let id_str = id.0.to_string();
            for label in &removed {
                let label_key = self.kp(key_label(graph, label));
                self.call_cmd(ctx, "SREM", &[&label_key, &id_str])?;
            }
            Ok(())
        })
    }

    pub(super) fn impl_delete_node(
        &self,
        graph: &str,
        id: NodeId,
        detach: bool,
    ) -> Result<(), StorageError> {
        let node = self
            .impl_get_node(graph, id)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;

        // Check for edges if not detaching.
        if !detach {
            let out = self.impl_out_edges(graph, id)?;
            let in_ = self.impl_in_edges(graph, id)?;
            if !out.is_empty() || !in_.is_empty() {
                return Err(StorageError::Backend {
                    msg: format!("node {} has relationships; use DETACH DELETE", id.0),
                });
            }
        }

        // Detach: delete all adjacent edges first.
        if detach {
            let mut edge_ids: std::collections::HashSet<cypher_core::storage::EdgeId> =
                std::collections::HashSet::new();
            for entry in self.impl_out_edges(graph, id)? {
                edge_ids.insert(entry.edge_id);
            }
            for entry in self.impl_in_edges(graph, id)? {
                edge_ids.insert(entry.edge_id);
            }
            for eid in edge_ids {
                self.impl_delete_edge(graph, eid)?;
            }
        }

        self.with_ctx(|ctx| {
            let id_str = id.0.to_string();

            // SREM from label indexes.
            for label in &node.labels {
                let label_key = self.kp(key_label(graph, label));
                self.call_cmd(ctx, "SREM", &[&label_key, &id_str])?;
            }

            // SREM from all-nodes set.
            let nodes_set_key = self.kp(key_nodes_set(graph));
            self.call_cmd(ctx, "SREM", &[&nodes_set_key, &id_str])?;

            // DEL adjacency keys.
            let out_key = self.kp(key_out(graph, id.0));
            self.call_cmd(ctx, "DEL", &[&out_key])?;
            let in_key = self.kp(key_in(graph, id.0));
            self.call_cmd(ctx, "DEL", &[&in_key])?;

            // DEL node hash.
            let node_key = self.kp(key_node(graph, id.0));
            self.call_cmd(ctx, "DEL", &[&node_key])?;

            Ok(())
        })
    }
}
