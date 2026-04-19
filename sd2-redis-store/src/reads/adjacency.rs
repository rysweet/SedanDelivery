// Adjacency reads: out/in edges and the batched adjacency lookup.

use cypher_core::error::StorageError;
use cypher_core::storage::{AdjEntry, EdgeId, NodeId};

use crate::keys::{key_in, key_out};
use crate::RedisGraphStore;

use super::helpers::{smembers_or_scan, smembers_threshold, sscan_all_members};

impl RedisGraphStore {
    pub(crate) fn out_edges_impl(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let mut conn = self.route_read_for_graph(graph)?;
        let members = smembers_or_scan(
            conn.as_mut(),
            &self.kp(key_out(graph, node_id.0)),
            smembers_threshold(),
        )?;
        Ok(Self::parse_adj_members(members))
    }

    pub(crate) fn in_edges_impl(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let mut conn = self.route_read_for_graph(graph)?;
        let members = smembers_or_scan(
            conn.as_mut(),
            &self.kp(key_in(graph, node_id.0)),
            smembers_threshold(),
        )?;
        Ok(Self::parse_adj_members(members))
    }

    pub(crate) fn out_edges_by_type_impl(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let all = self.out_edges_impl(graph, node_id)?;
        if rel_types.is_empty() {
            return Ok(all);
        }
        Ok(all
            .into_iter()
            .filter(|e| rel_types.contains(&e.rel_type.as_str()))
            .collect())
    }

    pub(crate) fn in_edges_by_type_impl(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let all = self.in_edges_impl(graph, node_id)?;
        if rel_types.is_empty() {
            return Ok(all);
        }
        Ok(all
            .into_iter()
            .filter(|e| rel_types.contains(&e.rel_type.as_str()))
            .collect())
    }

    pub(crate) fn out_edges_by_type_batch_impl(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        self.adjacency_batch_impl(graph, node_ids, false, rel_types)
    }

    pub(crate) fn in_edges_by_type_batch_impl(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        self.adjacency_batch_impl(graph, node_ids, true, rel_types)
    }

    fn adjacency_batch_impl(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        incoming: bool,
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        if node_ids.is_empty() {
            return Ok(vec![]);
        }

        let mut conn = self.route_read_for_graph(graph)?;
        let threshold = smembers_threshold();
        let keys: Vec<String> = node_ids
            .iter()
            .map(|node_id| {
                let key = if incoming {
                    key_in(graph, node_id.0)
                } else {
                    key_out(graph, node_id.0)
                };
                self.kp(key)
            })
            .collect();

        let mut scard_pipe = conn.pipeline();
        for key in &keys {
            scard_pipe.cmd("SCARD").arg(key);
        }
        let cards: Vec<u64> = scard_pipe
            .query(&mut conn)
            .map_err(crate::backend_storage_error)?;

        let mut results = vec![Vec::new(); node_ids.len()];
        let mut small_indexes = Vec::new();
        let mut small_pipe = conn.pipeline();
        for (idx, (key, card)) in keys.iter().zip(&cards).enumerate() {
            if *card <= threshold {
                small_indexes.push(idx);
                small_pipe.cmd("SMEMBERS").arg(key);
            }
        }

        if !small_indexes.is_empty() {
            let raw_batches: Vec<Vec<String>> = small_pipe
                .query(&mut conn)
                .map_err(crate::backend_storage_error)?;
            for (batch_idx, row_idx) in small_indexes.into_iter().enumerate() {
                let mut entries = Self::parse_adj_members(raw_batches[batch_idx].clone());
                if !rel_types.is_empty() {
                    entries.retain(|entry| rel_types.contains(&entry.rel_type.as_str()));
                }
                results[row_idx] = entries;
            }
        }

        for (idx, (key, card)) in keys.iter().zip(&cards).enumerate() {
            if *card > threshold {
                let mut entries =
                    Self::parse_adj_members(sscan_all_members(conn.as_mut(), key, *card as usize)?);
                if !rel_types.is_empty() {
                    entries.retain(|entry| rel_types.contains(&entry.rel_type.as_str()));
                }
                results[idx] = entries;
            }
        }

        Ok(results)
    }

    pub(crate) fn out_edge_ids_impl(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<EdgeId>, StorageError> {
        Ok(self
            .out_edges_impl(graph, node_id)?
            .into_iter()
            .map(|e| e.edge_id)
            .collect())
    }

    pub(crate) fn in_edge_ids_impl(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<EdgeId>, StorageError> {
        Ok(self
            .in_edges_impl(graph, node_id)?
            .into_iter()
            .map(|e| e.edge_id)
            .collect())
    }
}
