// writes/graphs.rs -- Graph lifecycle (create/delete) operations for RedisGraphStore.

use gql_core::error::StorageError;
use gql_core::serde_value::json_to_props;
use gql_core::storage::validate_graph_name;

use crate::keys::{
    key_edge, key_edge_seq, key_edges_set, key_graph_deleting, key_in, key_indexes, key_label,
    key_label_names, key_node, key_node_seq, key_nodes_set, key_out, key_propidx,
    key_propidx_manifest, key_reltype, key_reltype_names, value_to_index_key, IDX_SEP,
};
use crate::lock::WRITE_LOCK_TTL_MS;
use crate::RedisGraphStore;

impl RedisGraphStore {
    /// Create a new named graph in the registry.
    ///
    /// Validates the graph name, then adds it to the graph registry set.
    /// Returns an error if the graph already exists.
    pub(crate) fn create_graph_impl(&self, name: &str) -> Result<(), StorageError> {
        validate_graph_name(name)?;
        self.with_graph_write_lock(name, WRITE_LOCK_TTL_MS, 3, 50, |conn| {
            if self.graph_is_deleting_on(conn.as_mut(), name)? {
                return Err(StorageError::TransactionConflict);
            }
            let added: i64 = redis::cmd("SADD")
                .arg(self.key_graphs())
                .arg(name)
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;
            if added == 0 {
                return Err(StorageError::GraphAlreadyExists {
                    name: name.to_string(),
                });
            }
            Ok(())
        })
    }

    /// Delete a graph and all its data from Redis.
    ///
    /// This implementation is cluster-safe: it never uses SCAN (which cannot
    /// cross slot boundaries).  Instead it:
    ///
    /// 1. Reads the all-nodes set (`key_nodes_set`) and all-edges set
    ///    (`key_edges_set`) via SMEMBERS — safe because both keys share the
    ///    `{name}` hash tag and therefore live on the same slot.
    /// 2. Rejects graphs whose node or edge manifests exceed
    ///    `crate::MAX_NODES_FOR_DELETE`, keeping cleanup memory bounded.
    /// 3. Reconstructs every per-graph key that can exist for the graph,
    ///    including label/reltype registries and property-index keys.
    /// 4. Sets a per-graph `_deleting` marker so public reads stop treating the
    ///    graph as live while cleanup is in progress, but the name remains
    ///    reserved for retry after partial failures.
    /// 5. Builds a single DEL command covering every per-graph key derived from
    ///    the enumerated node and edge IDs, plus the well-known fixed keys
    ///    (sequences, index registry, nodes/edges sets, propidx manifest), and
    ///    executes it in one cluster-safe pipeline round-trip.
    /// 6. Only after the per-graph keys are gone, removes the graph from the
    ///    global registry and clears the `_deleting` marker.
    ///
    /// If cleanup fails after the `_deleting` marker is set, callers see
    /// `GraphNotFound` but the name remains reserved and `delete_graph` can be
    /// retried to finish cleanup safely.
    pub(crate) fn delete_graph_impl(&self, name: &str) -> Result<(), StorageError> {
        self.with_graph_write_lock(name, WRITE_LOCK_TTL_MS * 12, 10, 100, |conn| {
            let retrying_delete = self.graph_is_deleting_on(conn.as_mut(), name)?;
            if !retrying_delete {
                self.require_graph_on(conn.as_mut(), name)?;
            }

            // Enumerate all node IDs before deregistering the graph so an oversized
            // graph cannot become unreachable if the size guard fires.
            let node_ids: Vec<String> = redis::cmd("SMEMBERS")
                .arg(self.kp(key_nodes_set(name)))
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;

            if node_ids.len() > crate::MAX_NODES_FOR_DELETE {
                return Err(StorageError::Backend {
                    msg: format!(
                        "graph '{name}' has {} nodes, exceeding MAX_NODES_FOR_DELETE ({}); \
                         delete in parts or use an offline tool",
                        node_ids.len(),
                        crate::MAX_NODES_FOR_DELETE
                    ),
                });
            }

            // Enumerate all edge IDs from the all-edges set.
            let edge_ids: Vec<String> = redis::cmd("SMEMBERS")
                .arg(self.kp(key_edges_set(name)))
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;
            if edge_ids.len() > crate::MAX_NODES_FOR_DELETE {
                return Err(StorageError::Backend {
                    msg: format!(
                        "graph '{name}' has {} edges, exceeding MAX_NODES_FOR_DELETE ({}); \
                         delete in parts or use an offline tool",
                        edge_ids.len(),
                        crate::MAX_NODES_FOR_DELETE
                    ),
                });
            }

            let index_specs: Vec<String> = redis::cmd("SMEMBERS")
                .arg(self.kp(key_indexes(name)))
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;
            let label_registry_members: Vec<String> = redis::cmd("SMEMBERS")
                .arg(self.kp(key_label_names(name)))
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;
            let reltype_registry_members: Vec<String> = redis::cmd("SMEMBERS")
                .arg(self.kp(key_reltype_names(name)))
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;
            let propidx_manifest_members: Vec<String> = redis::cmd("SMEMBERS")
                .arg(self.kp(key_propidx_manifest(name)))
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;

            let parsed_indexes: Vec<(String, String)> = index_specs
                .iter()
                .filter_map(|spec| {
                    spec.split_once(IDX_SEP)
                        .map(|(label, property)| (label.to_string(), property.to_string()))
                })
                .collect();
            let mut label_names: std::collections::HashSet<String> =
                label_registry_members.into_iter().collect();
            let mut reltype_names: std::collections::HashSet<String> =
                reltype_registry_members.into_iter().collect();
            let mut propidx_keys = std::collections::HashSet::new();
            for (label, _) in &parsed_indexes {
                label_names.insert(label.clone());
            }
            for chunk in node_ids.chunks(500) {
                conn.renew_lock()?;
                let mut valid_ids = Vec::new();
                let mut pipe = conn.pipeline();
                for id_str in chunk {
                    if let Ok(id) = id_str.parse::<u64>() {
                        valid_ids.push(id_str.as_str());
                        pipe.cmd("HMGET")
                            .arg(self.kp(key_node(name, id)))
                            .arg("labels")
                            .arg("props");
                    }
                }
                if valid_ids.is_empty() {
                    continue;
                }
                let results: Vec<Vec<Option<String>>> =
                    pipe.query(conn).map_err(crate::backend_storage_error)?;
                for (_id_str, fields) in valid_ids.into_iter().zip(results.into_iter()) {
                    let Some(labels_json) = fields.first().and_then(|value| value.as_ref()) else {
                        continue;
                    };
                    let Some(props_json) = fields.get(1).and_then(|value| value.as_ref()) else {
                        continue;
                    };
                    let labels: Vec<String> = serde_json::from_str(labels_json).map_err(|e| {
                        StorageError::Serialization {
                            msg: format!("delete_graph labels decode: {e}"),
                        }
                    })?;
                    for label in &labels {
                        label_names.insert(label.clone());
                    }
                    let props =
                        json_to_props(props_json).map_err(|e| StorageError::Serialization {
                            msg: format!("delete_graph props decode: {e}"),
                        })?;
                    for (label, property) in &parsed_indexes {
                        if labels.iter().any(|node_label| node_label == label) {
                            if let Some(value) = props.get(property) {
                                if !value.is_null() {
                                    propidx_keys.insert(self.kp(key_propidx(
                                        name,
                                        label,
                                        property,
                                        &value_to_index_key(value),
                                    )));
                                }
                            }
                        }
                    }
                }
            }
            for chunk in edge_ids.chunks(500) {
                conn.renew_lock()?;
                let mut pipe = conn.pipeline();
                let mut has_cmds = false;
                for id_str in chunk {
                    if let Ok(id) = id_str.parse::<u64>() {
                        pipe.cmd("HGET")
                            .arg(self.kp(key_edge(name, id)))
                            .arg("rel_type");
                        has_cmds = true;
                    }
                }
                if !has_cmds {
                    continue;
                }
                let results: Vec<Option<String>> =
                    pipe.query(conn).map_err(crate::backend_storage_error)?;
                reltype_names.extend(results.into_iter().flatten());
            }

            // Build the list of all per-graph keys to delete.
            // Fixed keys (sequences, set manifests, registries, propidx manifest).
            let deleting_key = self.kp(key_graph_deleting(name));
            conn.renew_lock()?;
            redis::cmd("SET")
                .arg(&deleting_key)
                .arg("1")
                .query::<()>(conn.as_mut())
                .map_err(crate::backend_storage_error)?;
            let mut keys: Vec<String> = vec![
                self.kp(key_node_seq(name)),
                self.kp(key_edge_seq(name)),
                self.kp(key_nodes_set(name)),
                self.kp(key_edges_set(name)),
                self.kp(key_indexes(name)),
                self.kp(key_label_names(name)),
                self.kp(key_reltype_names(name)),
                self.kp(key_propidx_manifest(name)),
            ];

            // Per-node keys: node hash + out-adjacency + in-adjacency.
            for id_str in &node_ids {
                if let Ok(id) = id_str.parse::<u64>() {
                    keys.push(self.kp(key_node(name, id)));
                    keys.push(self.kp(key_out(name, id)));
                    keys.push(self.kp(key_in(name, id)));
                }
            }

            // Per-edge keys: edge hash.
            for id_str in &edge_ids {
                if let Ok(id) = id_str.parse::<u64>() {
                    keys.push(self.kp(key_edge(name, id)));
                }
            }
            for label in label_names {
                keys.push(self.kp(key_label(name, &label)));
            }
            for reltype in reltype_names {
                keys.push(self.kp(key_reltype(name, &reltype)));
            }
            keys.extend(propidx_keys);
            keys.extend(propidx_manifest_members);
            keys.sort_unstable();
            keys.dedup();

            // Delete all keys in a single cluster-safe pipeline round-trip.
            // All keys share the {name} hash tag so they live on the same slot.
            if !keys.is_empty() {
                conn.renew_lock()?;
                let mut del_cmd = redis::cmd("DEL");
                for k in &keys {
                    del_cmd.arg(k);
                }
                del_cmd
                    .query::<()>(conn.as_mut())
                    .map_err(crate::backend_storage_error)?;
            }

            // Keep the graph name reserved until the per-graph keys are gone.
            conn.renew_lock()?;
            redis::cmd("SREM")
                .arg(self.key_graphs())
                .arg(name)
                .query::<()>(conn.as_mut())
                .map_err(crate::backend_storage_error)?;
            redis::cmd("DEL")
                .arg(&deleting_key)
                .query::<()>(conn.as_mut())
                .map_err(crate::backend_storage_error)?;

            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use gql_core::storage::validate_graph_name;

    // ---- Graph name validation (pure logic, no Redis needed) ----------------

    #[test]
    fn valid_graph_names() {
        assert!(validate_graph_name("mydb").is_ok());
        assert!(validate_graph_name("test_graph").is_ok());
        assert!(validate_graph_name("g1").is_ok());
        assert!(validate_graph_name("a").is_ok());
    }

    #[test]
    fn empty_graph_name_rejected() {
        assert!(validate_graph_name("").is_err());
    }

    // ---- delete_graph key construction (set-enumeration path) ---------------

    #[test]
    fn delete_graph_fixed_keys_carry_hash_tag() {
        // The fixed keys built by delete_graph_impl must all include the
        // {name} hash tag so they route to the same Redis Cluster slot.
        use crate::keys::{
            key_edge_seq, key_edges_set, key_graph_deleting, key_indexes, key_label_names,
            key_node_seq, key_nodes_set, key_propidx_manifest, key_reltype_names,
        };
        let name = "testdb";
        let tag = format!("{{{name}}}");
        for key in [
            key_node_seq(name),
            key_edge_seq(name),
            key_nodes_set(name),
            key_edges_set(name),
            key_indexes(name),
            key_graph_deleting(name),
            key_label_names(name),
            key_reltype_names(name),
            key_propidx_manifest(name),
        ] {
            assert!(
                key.contains(&tag),
                "key '{key}' must contain hash tag '{tag}'"
            );
        }
    }

    #[test]
    fn delete_graph_derived_keys_carry_hash_tag() {
        // Per-node, per-edge, registry-backed, and propidx keys derived during
        // delete_graph_impl must all include the graph hash tag.
        use crate::keys::{
            key_edge, key_in, key_label, key_node, key_out, key_propidx, key_reltype,
        };
        let name = "testdb";
        let tag = format!("{{{name}}}");
        for key in [
            key_node(name, 1),
            key_out(name, 1),
            key_in(name, 1),
            key_edge(name, 1),
            key_label(name, "Person"),
            key_reltype(name, "KNOWS"),
            key_propidx(name, "Person", "name", "s:Alice"),
        ] {
            assert!(
                key.contains(&tag),
                "key '{key}' must contain hash tag '{tag}'"
            );
        }
    }

    #[test]
    fn max_nodes_for_delete_constant() {
        // Verify the guard is a positive, bounded value.
        assert!(crate::MAX_NODES_FOR_DELETE > 0);
        assert!(crate::MAX_NODES_FOR_DELETE <= 1_000_000);
    }

    #[test]
    fn delete_graph_checks_size_before_registry_removal() {
        let source = include_str!("graphs.rs");
        let delete_impl = source
            .split("pub(crate) fn delete_graph_impl")
            .nth(1)
            .expect("delete_graph_impl should exist");
        let size_guard = delete_impl
            .find("if node_ids.len() > crate::MAX_NODES_FOR_DELETE")
            .expect("size guard should exist");
        let deregister = delete_impl
            .rfind("redis::cmd(\"SREM\")")
            .expect("graph deregistration should exist");
        assert!(
            size_guard < deregister,
            "delete_graph_impl must reject oversized graphs before removing them from the registry"
        );
    }

    #[test]
    fn delete_graph_checks_edge_manifest_size() {
        let source = include_str!("graphs.rs");
        let delete_impl = source
            .split("pub(crate) fn delete_graph_impl")
            .nth(1)
            .expect("delete_graph_impl should exist");
        assert!(
            delete_impl.contains("if edge_ids.len() > crate::MAX_NODES_FOR_DELETE"),
            "delete_graph_impl must bound the edge manifest before building cleanup state"
        );
    }
}
