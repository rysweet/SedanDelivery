// Whole-graph enumeration: all node ids, edge ids, and label names.

use cypher_core::error::StorageError;
use cypher_core::storage::{EdgeId, NodeId};

use crate::keys::{key_edges_set, key_nodes_set};
use crate::RedisGraphStore;

use super::helpers::{smembers_or_scan, smembers_threshold};

impl RedisGraphStore {
    pub(crate) fn all_node_ids_impl(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        let mut conn = self.route_read_for_graph(graph)?;
        let members = smembers_or_scan(
            conn.as_mut(),
            &self.kp(key_nodes_set(graph)),
            smembers_threshold(),
        )?;
        members
            .into_iter()
            .map(|s| {
                s.parse::<u64>()
                    .map(NodeId)
                    .map_err(|_| StorageError::Serialization {
                        msg: format!("bad node id in nodes set: {s}"),
                    })
            })
            .collect()
    }

    pub(crate) fn all_edge_ids_impl(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        let mut conn = self.route_read_for_graph(graph)?;
        let members = smembers_or_scan(
            conn.as_mut(),
            &self.kp(key_edges_set(graph)),
            smembers_threshold(),
        )?;
        members
            .into_iter()
            .map(|s| {
                s.parse::<u64>()
                    .map(EdgeId)
                    .map_err(|_| StorageError::Serialization {
                        msg: format!("bad edge id in edges set: {s}"),
                    })
            })
            .collect()
    }

    pub(crate) fn all_label_names_impl(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        let mut conn = self.route_read_for_graph(graph)?;
        self.active_label_names(&mut conn, graph)
    }
}
