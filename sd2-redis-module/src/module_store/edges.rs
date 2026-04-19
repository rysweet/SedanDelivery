//! Edge-level operations for `ModuleRedisStore`.
//!
//! These are inherent methods invoked by the `GraphStore` trait impl in
//! `mod.rs`. Each method mirrors the corresponding trait method's signature.

use indexmap::IndexMap;

use cypher_core::error::StorageError;
use cypher_core::serde_value::props_to_json;
use cypher_core::storage::{EdgeData, EdgeId, NodeId};
use cypher_core::types::Value;

use super::keys::{
    encode_adj, key_edge, key_edge_seq, key_edges_set, key_in, key_node, key_out, key_reltype,
    key_reltype_names,
};
use super::ModuleRedisStore;

impl ModuleRedisStore {
    pub(super) fn impl_get_edge(
        &self,
        graph: &str,
        id: EdgeId,
    ) -> Result<Option<EdgeData>, StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_edge(graph, id.0));
            let result = self.call_cmd(ctx, "HMGET", &[&key, "src", "dst", "rel_type", "props"])?;
            let fields = Self::extract_hmget_fields(&result);
            self.parse_edge_from_hmget(id, &fields)
        })
    }

    pub(super) fn impl_find_edges_by_type(
        &self,
        graph: &str,
        rel_type: &str,
    ) -> Result<Vec<EdgeId>, StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_reltype(graph, rel_type));
            let result = self.call_cmd(ctx, "SMEMBERS", &[&key])?;
            Self::extract_string_array(&result)
                .into_iter()
                .map(|s| {
                    s.parse::<u64>()
                        .map(EdgeId)
                        .map_err(|_| StorageError::Serialization {
                            msg: format!("bad edge id in reltype index: {s}"),
                        })
                })
                .collect()
        })
    }

    pub(super) fn impl_all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_edges_set(graph));
            let result = self.call_cmd(ctx, "SMEMBERS", &[&key])?;
            Self::extract_string_array(&result)
                .into_iter()
                .map(|s| {
                    s.parse::<u64>()
                        .map(EdgeId)
                        .map_err(|_| StorageError::Serialization {
                            msg: format!("bad edge id in edges set: {s}"),
                        })
                })
                .collect()
        })
    }

    pub(super) fn impl_create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        self.with_ctx(|ctx| {
            // Verify src and dst exist.
            let src_key = self.kp(key_node(graph, src.0));
            let src_check = self.call_cmd(ctx, "EXISTS", &[&src_key])?;
            if Self::expect_integer(&src_check, "EXISTS")? == 0 {
                return Err(StorageError::NodeNotFound { id: src.0 });
            }
            let dst_key = self.kp(key_node(graph, dst.0));
            let dst_check = self.call_cmd(ctx, "EXISTS", &[&dst_key])?;
            if Self::expect_integer(&dst_check, "EXISTS")? == 0 {
                return Err(StorageError::NodeNotFound { id: dst.0 });
            }

            // Allocate ID.
            let seq_key = self.kp(key_edge_seq(graph));
            let result = self.call_cmd(ctx, "INCRBY", &[&seq_key, "1"])?;
            let raw_id = Self::expect_integer(&result, "INCRBY")?;
            if raw_id <= 0 {
                return Err(StorageError::IdOverflow {
                    graph: graph.to_string(),
                });
            }
            let id = raw_id as u64;
            let id_str = id.to_string();

            // Serialize.
            let props_json = props_to_json(&properties)?;

            // HSET edge hash.
            let edge_key = self.kp(key_edge(graph, id));
            let src_str = src.0.to_string();
            let dst_str = dst.0.to_string();
            self.call_cmd(
                ctx,
                "HSET",
                &[
                    &edge_key,
                    "src",
                    &src_str,
                    "dst",
                    &dst_str,
                    "rel_type",
                    &rel_type,
                    "props",
                    &props_json,
                ],
            )?;

            // SADD to all-edges set.
            let edges_set_key = self.kp(key_edges_set(graph));
            self.call_cmd(ctx, "SADD", &[&edges_set_key, &id_str])?;

            // SADD to reltype index.
            let reltype_key = self.kp(key_reltype(graph, &rel_type));
            self.call_cmd(ctx, "SADD", &[&reltype_key, &id_str])?;

            // SADD reltype name to registry.
            let reltype_names_key = self.kp(key_reltype_names(graph));
            self.call_cmd(ctx, "SADD", &[&reltype_names_key, &rel_type])?;

            // SADD out-adjacency.
            let out_key = self.kp(key_out(graph, src.0));
            let out_member = encode_adj(EdgeId(id), dst, &rel_type);
            self.call_cmd(ctx, "SADD", &[&out_key, &out_member])?;

            // SADD in-adjacency.
            let in_key = self.kp(key_in(graph, dst.0));
            let in_member = encode_adj(EdgeId(id), src, &rel_type);
            self.call_cmd(ctx, "SADD", &[&in_key, &in_member])?;

            Ok(EdgeId(id))
        })
    }

    pub(super) fn impl_set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        let edge = self
            .impl_get_edge(graph, id)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        let mut merged = edge.properties;
        merged.extend(properties);
        let props_json = props_to_json(&merged)?;
        self.with_ctx(|ctx| {
            let edge_key = self.kp(key_edge(graph, id.0));
            self.call_cmd(ctx, "HSET", &[&edge_key, "props", &props_json])?;
            Ok(())
        })
    }

    pub(super) fn impl_replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        if self.impl_get_edge(graph, id)?.is_none() {
            return Err(StorageError::EdgeNotFound { id: id.0 });
        }
        let props_json = props_to_json(&properties)?;
        self.with_ctx(|ctx| {
            let edge_key = self.kp(key_edge(graph, id.0));
            self.call_cmd(ctx, "HSET", &[&edge_key, "props", &props_json])?;
            Ok(())
        })
    }

    pub(super) fn impl_remove_edge_property(
        &self,
        graph: &str,
        id: EdgeId,
        key: &str,
    ) -> Result<(), StorageError> {
        let edge = self
            .impl_get_edge(graph, id)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        let mut props = edge.properties;
        props.shift_remove(key);
        let props_json = props_to_json(&props)?;
        self.with_ctx(|ctx| {
            let edge_key = self.kp(key_edge(graph, id.0));
            self.call_cmd(ctx, "HSET", &[&edge_key, "props", &props_json])?;
            Ok(())
        })
    }

    pub(super) fn impl_delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        let edge = self
            .impl_get_edge(graph, id)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;

        self.with_ctx(|ctx| {
            let id_str = id.0.to_string();

            // SREM from reltype index.
            let reltype_key = self.kp(key_reltype(graph, &edge.rel_type));
            self.call_cmd(ctx, "SREM", &[&reltype_key, &id_str])?;

            // SREM from all-edges set.
            let edges_set_key = self.kp(key_edges_set(graph));
            self.call_cmd(ctx, "SREM", &[&edges_set_key, &id_str])?;

            // SREM out-adjacency.
            let out_key = self.kp(key_out(graph, edge.src.0));
            let out_member = encode_adj(id, edge.dst, &edge.rel_type);
            self.call_cmd(ctx, "SREM", &[&out_key, &out_member])?;

            // SREM in-adjacency.
            let in_key = self.kp(key_in(graph, edge.dst.0));
            let in_member = encode_adj(id, edge.src, &edge.rel_type);
            self.call_cmd(ctx, "SREM", &[&in_key, &in_member])?;

            // DEL edge hash.
            let edge_key = self.kp(key_edge(graph, id.0));
            self.call_cmd(ctx, "DEL", &[&edge_key])?;

            Ok(())
        })
    }
}
