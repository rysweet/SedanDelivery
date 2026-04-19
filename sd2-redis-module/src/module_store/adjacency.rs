//! Adjacency-set reads (out/in edges) for `ModuleRedisStore`.

use cypher_core::error::StorageError;
use cypher_core::storage::{AdjEntry, EdgeId, NodeId};

use super::keys::{key_in, key_out};
use super::ModuleRedisStore;

impl ModuleRedisStore {
    pub(super) fn impl_out_edges(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_out(graph, node_id.0));
            let result = self.call_cmd(ctx, "SMEMBERS", &[&key])?;
            let members = Self::extract_string_array(&result);
            Ok(Self::parse_adj_members(&members))
        })
    }

    pub(super) fn impl_in_edges(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_in(graph, node_id.0));
            let result = self.call_cmd(ctx, "SMEMBERS", &[&key])?;
            let members = Self::extract_string_array(&result);
            Ok(Self::parse_adj_members(&members))
        })
    }

    pub(super) fn impl_out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let all = self.impl_out_edges(graph, node_id)?;
        if rel_types.is_empty() {
            return Ok(all);
        }
        Ok(all
            .into_iter()
            .filter(|e| rel_types.contains(&e.rel_type.as_str()))
            .collect())
    }

    pub(super) fn impl_in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let all = self.impl_in_edges(graph, node_id)?;
        if rel_types.is_empty() {
            return Ok(all);
        }
        Ok(all
            .into_iter()
            .filter(|e| rel_types.contains(&e.rel_type.as_str()))
            .collect())
    }

    pub(super) fn impl_out_edge_ids(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<EdgeId>, StorageError> {
        Ok(self
            .impl_out_edges(graph, node_id)?
            .into_iter()
            .map(|e| e.edge_id)
            .collect())
    }

    pub(super) fn impl_in_edge_ids(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<EdgeId>, StorageError> {
        Ok(self
            .impl_in_edges(graph, node_id)?
            .into_iter()
            .map(|e| e.edge_id)
            .collect())
    }
}
