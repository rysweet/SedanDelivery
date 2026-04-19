// Graph administration reads: existence, listing, statistics.

use cypher_core::error::StorageError;
use cypher_core::storage::GraphStats;

use crate::keys::{key_edges_set, key_nodes_set};
use crate::RedisGraphStore;

use super::helpers::{smembers_or_scan, smembers_threshold};

impl RedisGraphStore {
    pub(crate) fn graph_exists_impl(&self, name: &str) -> Result<bool, StorageError> {
        let mut conn = self.route_read()?;
        let exists = self.graph_exists_on(conn.as_mut(), name)?;
        if exists || self.replica_pool.is_none() {
            return Ok(exists);
        }

        let mut primary = self.route_read_with_pinning(true)?;
        self.graph_exists_on(primary.as_mut(), name)
    }

    pub(crate) fn list_graphs_impl(&self) -> Result<Vec<String>, StorageError> {
        let mut conn = self.route_read()?;
        let mut names = Vec::new();
        for name in smembers_or_scan(conn.as_mut(), &self.key_graphs(), smembers_threshold())? {
            if !self.graph_is_deleting_on(conn.as_mut(), &name)? {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    pub(crate) fn graph_stats_impl(&self, graph: &str) -> Result<GraphStats, StorageError> {
        let mut conn = self.route_read_for_graph(graph)?;
        let node_count: u64 = redis::cmd("SCARD")
            .arg(self.kp(key_nodes_set(graph)))
            .query(conn.as_mut())
            .map_err(crate::backend_storage_error)?;
        let edge_count: u64 = redis::cmd("SCARD")
            .arg(self.kp(key_edges_set(graph)))
            .query(conn.as_mut())
            .map_err(crate::backend_storage_error)?;
        let label_count = self.active_label_names(&mut conn, graph)?.len();
        let rel_type_count = self.active_reltype_names(&mut conn, graph)?.len();
        let label_count_u32 = u32::try_from(label_count).unwrap_or_else(|_| {
            tracing::warn!(
                graph,
                label_count,
                "label_count exceeds u32::MAX, clamping to u32::MAX"
            );
            u32::MAX
        });
        let rel_type_count_u32 = u32::try_from(rel_type_count).unwrap_or_else(|_| {
            tracing::warn!(
                graph,
                rel_type_count,
                "rel_type_count exceeds u32::MAX, clamping to u32::MAX"
            );
            u32::MAX
        });
        Ok(GraphStats {
            node_count,
            edge_count,
            label_count: label_count_u32,
            rel_type_count: rel_type_count_u32,
        })
    }
}
