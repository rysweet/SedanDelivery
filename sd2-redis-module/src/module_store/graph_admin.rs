//! Graph-level administration: create/delete/list/stats.

use cypher_core::error::StorageError;
use cypher_core::storage::GraphStats;

use super::keys::{
    key_edge, key_edge_seq, key_edges_set, key_graph_deleting, key_in, key_indexes, key_label,
    key_label_names, key_node, key_node_seq, key_nodes_set, key_out, key_reltype,
    key_reltype_names, KEY_GRAPHS,
};
use super::ModuleRedisStore;

impl ModuleRedisStore {
    pub(super) fn impl_create_graph(&self, name: &str) -> Result<(), StorageError> {
        cypher_core::storage::validate_graph_name(name)?;
        self.with_ctx(|ctx| {
            // Check if graph is being deleted.
            let deleting_key = self.kp(key_graph_deleting(name));
            let exists = self.call_cmd(ctx, "EXISTS", &[&deleting_key])?;
            if Self::expect_integer(&exists, "EXISTS")? > 0 {
                return Err(StorageError::TransactionConflict);
            }

            let graphs_key = self.kp(KEY_GRAPHS.to_string());
            let result = self.call_cmd(ctx, "SADD", &[&graphs_key, name])?;
            let added = Self::expect_integer(&result, "SADD")?;
            if added == 0 {
                return Err(StorageError::GraphAlreadyExists {
                    name: name.to_string(),
                });
            }
            Ok(())
        })
    }

    pub(super) fn impl_delete_graph(&self, name: &str) -> Result<(), StorageError> {
        // Simplified delete: remove the graph from the registry and clean up
        // known keys. For large graphs, a more incremental approach is needed.
        self.with_ctx(|ctx| {
            // Verify graph exists.
            let graphs_key = self.kp(KEY_GRAPHS.to_string());
            let check = self.call_cmd(ctx, "SISMEMBER", &[&graphs_key, name])?;
            let is_member = Self::expect_integer(&check, "SISMEMBER")?;

            let deleting_key = self.kp(key_graph_deleting(name));
            let retrying = {
                let r = self.call_cmd(ctx, "EXISTS", &[&deleting_key])?;
                Self::expect_integer(&r, "EXISTS")? > 0
            };

            if is_member == 0 && !retrying {
                return Err(StorageError::GraphNotFound {
                    name: name.to_string(),
                });
            }

            // Set deleting marker.
            self.call_cmd(ctx, "SET", &[&deleting_key, "1"])?;

            // Enumerate all nodes and edges.
            let nodes_key = self.kp(key_nodes_set(name));
            let node_result = self.call_cmd(ctx, "SMEMBERS", &[&nodes_key])?;
            let node_ids = Self::extract_string_array(&node_result);

            let edges_key = self.kp(key_edges_set(name));
            let edge_result = self.call_cmd(ctx, "SMEMBERS", &[&edges_key])?;
            let edge_ids = Self::extract_string_array(&edge_result);

            // Collect label and reltype names.
            let labels_reg_key = self.kp(key_label_names(name));
            let labels_result = self.call_cmd(ctx, "SMEMBERS", &[&labels_reg_key])?;
            let label_names = Self::extract_string_array(&labels_result);

            let reltypes_reg_key = self.kp(key_reltype_names(name));
            let reltypes_result = self.call_cmd(ctx, "SMEMBERS", &[&reltypes_reg_key])?;
            let reltype_names = Self::extract_string_array(&reltypes_result);

            // Delete per-node keys.
            for id_str in &node_ids {
                if let Ok(id) = id_str.parse::<u64>() {
                    let nk = self.kp(key_node(name, id));
                    self.call_cmd(ctx, "DEL", &[&nk])?;
                    let ok = self.kp(key_out(name, id));
                    self.call_cmd(ctx, "DEL", &[&ok])?;
                    let ik = self.kp(key_in(name, id));
                    self.call_cmd(ctx, "DEL", &[&ik])?;
                }
            }

            // Delete per-edge keys.
            for id_str in &edge_ids {
                if let Ok(id) = id_str.parse::<u64>() {
                    let ek = self.kp(key_edge(name, id));
                    self.call_cmd(ctx, "DEL", &[&ek])?;
                }
            }

            // Delete label index sets.
            for label in &label_names {
                let lk = self.kp(key_label(name, label));
                self.call_cmd(ctx, "DEL", &[&lk])?;
            }

            // Delete reltype index sets.
            for reltype in &reltype_names {
                let rk = self.kp(key_reltype(name, reltype));
                self.call_cmd(ctx, "DEL", &[&rk])?;
            }

            // Delete fixed keys.
            for key in [
                self.kp(key_node_seq(name)),
                self.kp(key_edge_seq(name)),
                self.kp(key_nodes_set(name)),
                self.kp(key_edges_set(name)),
                self.kp(key_indexes(name)),
                self.kp(key_label_names(name)),
                self.kp(key_reltype_names(name)),
            ] {
                self.call_cmd(ctx, "DEL", &[&key])?;
            }

            // Remove from registry and clear deleting marker.
            self.call_cmd(ctx, "SREM", &[&graphs_key, name])?;
            self.call_cmd(ctx, "DEL", &[&deleting_key])?;

            Ok(())
        })
    }

    pub(super) fn impl_graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        self.with_ctx(|ctx| {
            let graphs_key = self.kp(KEY_GRAPHS.to_string());
            let result = self.call_cmd(ctx, "SISMEMBER", &[&graphs_key, name])?;
            let is_member = Self::expect_integer(&result, "SISMEMBER")?;
            if is_member == 0 {
                return Ok(false);
            }
            // Check if graph is being deleted.
            let deleting_key = self.kp(key_graph_deleting(name));
            let exists = self.call_cmd(ctx, "EXISTS", &[&deleting_key])?;
            Ok(Self::expect_integer(&exists, "EXISTS")? == 0)
        })
    }

    pub(super) fn impl_list_graphs(&self) -> Result<Vec<String>, StorageError> {
        self.with_ctx(|ctx| {
            let graphs_key = self.kp(KEY_GRAPHS.to_string());
            let result = self.call_cmd(ctx, "SMEMBERS", &[&graphs_key])?;
            let mut names = Vec::new();
            for name in Self::extract_string_array(&result) {
                // Skip graphs that are being deleted.
                let deleting_key = self.kp(key_graph_deleting(&name));
                let exists = self.call_cmd(ctx, "EXISTS", &[&deleting_key])?;
                if Self::expect_integer(&exists, "EXISTS")? == 0 {
                    names.push(name);
                }
            }
            names.sort();
            Ok(names)
        })
    }

    pub(super) fn impl_graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        self.with_ctx(|ctx| {
            let nodes_key = self.kp(key_nodes_set(graph));
            let node_count_result = self.call_cmd(ctx, "SCARD", &[&nodes_key])?;
            let node_count = Self::expect_integer(&node_count_result, "SCARD")? as u64;

            let edges_key = self.kp(key_edges_set(graph));
            let edge_count_result = self.call_cmd(ctx, "SCARD", &[&edges_key])?;
            let edge_count = Self::expect_integer(&edge_count_result, "SCARD")? as u64;

            // Get label count from registry.
            let labels_reg_key = self.kp(key_label_names(graph));
            let labels_result = self.call_cmd(ctx, "SMEMBERS", &[&labels_reg_key])?;
            let label_names = Self::extract_string_array(&labels_result);
            // Count only labels that have at least one member.
            let mut active_labels = 0u32;
            for label in &label_names {
                let label_key = self.kp(key_label(graph, label));
                let card_result = self.call_cmd(ctx, "SCARD", &[&label_key])?;
                if Self::expect_integer(&card_result, "SCARD")? > 0 {
                    active_labels += 1;
                }
            }

            // Get reltype count from registry.
            let reltypes_reg_key = self.kp(key_reltype_names(graph));
            let reltypes_result = self.call_cmd(ctx, "SMEMBERS", &[&reltypes_reg_key])?;
            let reltype_names = Self::extract_string_array(&reltypes_result);
            let mut active_reltypes = 0u32;
            for reltype in &reltype_names {
                let reltype_key = self.kp(key_reltype(graph, reltype));
                let card_result = self.call_cmd(ctx, "SCARD", &[&reltype_key])?;
                if Self::expect_integer(&card_result, "SCARD")? > 0 {
                    active_reltypes += 1;
                }
            }

            Ok(GraphStats {
                node_count,
                edge_count,
                label_count: active_labels,
                rel_type_count: active_reltypes,
            })
        })
    }

    pub(super) fn impl_all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        self.with_ctx(|ctx| {
            let registry_key = self.kp(key_label_names(graph));
            let result = self.call_cmd(ctx, "SMEMBERS", &[&registry_key])?;
            let label_names = Self::extract_string_array(&result);

            // Filter to labels that have at least one member (active labels).
            let mut active = Vec::new();
            for label in label_names {
                let label_key = self.kp(key_label(graph, &label));
                let card_result = self.call_cmd(ctx, "SCARD", &[&label_key])?;
                if Self::expect_integer(&card_result, "SCARD")? > 0 {
                    active.push(label);
                }
            }
            active.sort_unstable();
            Ok(active)
        })
    }
}
