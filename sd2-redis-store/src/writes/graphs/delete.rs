// writes/graphs/delete.rs -- Cluster-safe graph deletion for RedisGraphStore.

use cypher_core::error::StorageError;

use crate::keys::{
    key_edge, key_edge_seq, key_edges_set, key_graph_deleting, key_in, key_indexes, key_label,
    key_label_names, key_node, key_node_seq, key_nodes_set, key_out, key_propidx_manifest,
    key_reltype, key_reltype_names, IDX_SEP,
};
use crate::lock::WRITE_LOCK_TTL_MS;
use crate::RedisGraphStore;

impl RedisGraphStore {
    /// Delete a graph and all its data from Redis.
    ///
    /// This implementation is cluster-safe: it never uses SCAN (which cannot
    /// cross slot boundaries).  Instead it:
    ///
    /// 1. Uses `SCARD` on the all-nodes set (`key_nodes_set`) and all-edges set
    ///    (`key_edges_set`) to reject oversized graphs before loading the full
    ///    manifests.
    /// 2. Only after both counts pass the size guard, reads the manifests via
    ///    `SMEMBERS` — safe because both keys share the `{name}` hash tag and
    ///    therefore live on the same slot.
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

            // Preflight the node manifest cardinality before loading all IDs so
            // oversized graphs fail cheaply and remain fully reachable.
            let node_count: usize = redis::cmd("SCARD")
                .arg(self.kp(key_nodes_set(name)))
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;

            if node_count > crate::MAX_NODES_FOR_DELETE {
                tracing::warn!(
                    graph = %name,
                    node_count,
                    limit = crate::MAX_NODES_FOR_DELETE,
                    "delete_graph rejected: node count exceeds MAX_NODES_FOR_DELETE"
                );
                return Err(StorageError::DeleteGraphTooLarge {
                    name: name.to_string(),
                    resource: "nodes",
                    resource_count: node_count,
                    limit: crate::MAX_NODES_FOR_DELETE,
                });
            }

            // Preflight the edge manifest cardinality before loading any
            // manifest so both oversized branches stay cheap and side-effect free.
            let edge_count: usize = redis::cmd("SCARD")
                .arg(self.kp(key_edges_set(name)))
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;
            if edge_count > crate::MAX_NODES_FOR_DELETE {
                tracing::warn!(
                    graph = %name,
                    edge_count,
                    limit = crate::MAX_NODES_FOR_DELETE,
                    "delete_graph rejected: edge count exceeds MAX_NODES_FOR_DELETE"
                );
                return Err(StorageError::DeleteGraphTooLarge {
                    name: name.to_string(),
                    resource: "edges",
                    resource_count: edge_count,
                    limit: crate::MAX_NODES_FOR_DELETE,
                });
            }

            let node_ids: Vec<String> = redis::cmd("SMEMBERS")
                .arg(self.kp(key_nodes_set(name)))
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;

            let edge_ids: Vec<String> = redis::cmd("SMEMBERS")
                .arg(self.kp(key_edges_set(name)))
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;

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
            self.delete_graph_collect_node_metadata(
                conn,
                name,
                &node_ids,
                &parsed_indexes,
                &mut label_names,
                &mut propidx_keys,
            )?;
            self.delete_graph_collect_edge_reltypes(conn, name, &edge_ids, &mut reltype_names)?;

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
            for (index, id_str) in node_ids.iter().enumerate() {
                if index > 0 && index % 250 == 0 {
                    conn.renew_lock()?;
                }
                match id_str.parse::<u64>() {
                    Ok(id) => {
                        keys.push(self.kp(key_node(name, id)));
                        keys.push(self.kp(key_out(name, id)));
                        keys.push(self.kp(key_in(name, id)));
                    }
                    Err(error) => {
                        tracing::warn!(
                            graph = %name,
                            node_id = %id_str,
                            %error,
                            "delete_graph omitting malformed node ID during key construction"
                        );
                    }
                }
            }

            // Per-edge keys: edge hash.
            for (index, id_str) in edge_ids.iter().enumerate() {
                if index > 0 && index % 250 == 0 {
                    conn.renew_lock()?;
                }
                match id_str.parse::<u64>() {
                    Ok(id) => {
                        keys.push(self.kp(key_edge(name, id)));
                    }
                    Err(error) => {
                        tracing::warn!(
                            graph = %name,
                            edge_id = %id_str,
                            %error,
                            "delete_graph omitting malformed edge ID during key construction"
                        );
                    }
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
