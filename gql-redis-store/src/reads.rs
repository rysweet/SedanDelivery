// reads.rs — All GraphStore read method implementations for RedisGraphStore.

use gql_core::error::StorageError;
use gql_core::serde_value::json_to_props;
use gql_core::storage::{AdjEntry, EdgeData, EdgeId, GraphStats, NodeData, NodeId, RangeQuery};

use crate::keys::{
    key_edge, key_edges_set, key_in, key_label, key_node, key_nodes_set, key_out, key_propidx,
    key_reltype, value_to_index_key, IDX_SEP,
};
use crate::range::{range_matches, scan_count, scan_key_count};
use crate::RedisGraphStore;

impl RedisGraphStore {
    // ── Single-item reads ────────────────────────────────────────────────────

    pub(crate) fn get_node_impl(
        &self,
        graph: &str,
        id: NodeId,
    ) -> Result<Option<NodeData>, StorageError> {
        let mut conn = self.route_read()?;
        self.read_node(conn.as_mut(), graph, id.0)
    }

    pub(crate) fn get_edge_impl(
        &self,
        graph: &str,
        id: EdgeId,
    ) -> Result<Option<EdgeData>, StorageError> {
        let mut conn = self.route_read()?;
        self.read_edge(conn.as_mut(), graph, id.0)
    }

    // ── Batch reads ──────────────────────────────────────────────────────────

    pub(crate) fn get_nodes_batch_impl(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let mut conn = self.route_read()?;
        let mut results = Vec::with_capacity(ids.len());
        // Pipeline all HMGET commands.
        let mut pipe = redis::pipe();
        for id in ids {
            pipe.cmd("HMGET")
                .arg(self.kp(key_node(graph, id.0)))
                .arg("labels")
                .arg("props");
        }
        let raw: Vec<Vec<Option<String>>> = pipe
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        for (idx, fields) in raw.into_iter().enumerate() {
            let id = ids[idx];
            match (
                &fields.first().and_then(|f| f.as_ref()),
                &fields.get(1).and_then(|f| f.as_ref()),
            ) {
                (Some(labels_json), Some(props_json)) => {
                    let labels: Vec<String> = serde_json::from_str(labels_json).map_err(|e| {
                        StorageError::Serialization {
                            msg: format!("labels batch: {e}"),
                        }
                    })?;
                    let properties = json_to_props(props_json)?;
                    results.push(Some(NodeData {
                        id,
                        labels,
                        properties,
                    }));
                }
                _ => results.push(None),
            }
        }
        Ok(results)
    }

    pub(crate) fn get_edges_batch_impl(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let mut conn = self.route_read()?;
        let mut pipe = redis::pipe();
        for id in ids {
            pipe.cmd("HMGET")
                .arg(self.kp(key_edge(graph, id.0)))
                .arg("src")
                .arg("dst")
                .arg("rel_type")
                .arg("props");
        }
        let raw: Vec<Vec<Option<String>>> = pipe
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        let mut results = Vec::with_capacity(ids.len());
        for (idx, fields) in raw.into_iter().enumerate() {
            let id = ids[idx];
            if fields.len() >= 4 {
                if let (Some(src_s), Some(dst_s), Some(rel_type), Some(props_json)) =
                    (&fields[0], &fields[1], &fields[2], &fields[3])
                {
                    let src =
                        NodeId(
                            src_s
                                .parse::<u64>()
                                .map_err(|_| StorageError::Serialization {
                                    msg: "bad src".into(),
                                })?,
                        );
                    let dst =
                        NodeId(
                            dst_s
                                .parse::<u64>()
                                .map_err(|_| StorageError::Serialization {
                                    msg: "bad dst".into(),
                                })?,
                        );
                    let properties = json_to_props(props_json)?;
                    results.push(Some(EdgeData {
                        id,
                        rel_type: rel_type.clone(),
                        src,
                        dst,
                        properties,
                    }));
                    continue;
                }
            }
            results.push(None);
        }
        Ok(results)
    }

    // ── Label/type index reads ───────────────────────────────────────────────

    pub(crate) fn find_nodes_by_label_impl(
        &self,
        graph: &str,
        label: &str,
    ) -> Result<Vec<NodeId>, StorageError> {
        let mut conn = self.route_read()?;
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_label(graph, label)))
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        members
            .into_iter()
            .map(|s| {
                s.parse::<u64>()
                    .map(NodeId)
                    .map_err(|_| StorageError::Serialization {
                        msg: format!("bad node id in label index: {s}"),
                    })
            })
            .collect()
    }

    pub(crate) fn find_nodes_by_label_and_property_impl(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        let mut conn = self.route_read()?;

        // Fast path: use property index for exact-match queries when an index exists.
        if let RangeQuery::Eq(val) = &range {
            let indexes = self.active_indexes(conn.as_mut(), graph)?;
            if indexes.contains(&format!("{label}{IDX_SEP}{property}")) {
                let vkey = value_to_index_key(val);
                let members: Vec<String> = redis::cmd("SMEMBERS")
                    .arg(self.kp(key_propidx(graph, label, property, &vkey)))
                    .query(conn.as_mut())
                    .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
                return members
                    .into_iter()
                    .map(|s| {
                        s.parse::<u64>()
                            .map(NodeId)
                            .map_err(|_| StorageError::Serialization {
                                msg: format!("bad node id in propidx: {s}"),
                            })
                    })
                    .collect();
            }
        }

        // Fallback: scan all nodes with the label and filter in-memory.
        let node_ids = self.find_nodes_by_label_impl(graph, label)?;
        let mut result = Vec::new();
        for id in node_ids {
            let node = self.read_node(conn.as_mut(), graph, id.0)?;
            if let Some(node) = node {
                if let Some(val) = node.properties.get(property) {
                    if range_matches(val, &range) {
                        result.push(id);
                    }
                }
            }
        }
        Ok(result)
    }

    pub(crate) fn find_edges_by_type_impl(
        &self,
        graph: &str,
        rel_type: &str,
    ) -> Result<Vec<EdgeId>, StorageError> {
        let mut conn = self.route_read()?;
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_reltype(graph, rel_type)))
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        members
            .into_iter()
            .map(|s| {
                s.parse::<u64>()
                    .map(EdgeId)
                    .map_err(|_| StorageError::Serialization {
                        msg: format!("bad edge id in reltype index: {s}"),
                    })
            })
            .collect()
    }

    // ── Adjacency reads ─────────────────────────────────────────────────────

    pub(crate) fn out_edges_impl(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let mut conn = self.route_read()?;
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_out(graph, node_id.0)))
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(Self::parse_adj_members(members))
    }

    pub(crate) fn in_edges_impl(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let mut conn = self.route_read()?;
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_in(graph, node_id.0)))
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
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

    pub(crate) fn all_node_ids_impl(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        let mut conn = self.route_read()?;
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_nodes_set(graph)))
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
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
        let mut conn = self.route_read()?;
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_edges_set(graph)))
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
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
        let prefix = format!("{}G:{{{graph}}}:label:", self.key_prefix);
        let pattern = format!("{prefix}*");
        let mut conn = self.route_read()?;
        let mut cursor: u64 = 0;
        let mut labels = Vec::new();
        loop {
            let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(scan_count())
                .query(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            for key in keys {
                if let Some(label) = key.strip_prefix(&prefix) {
                    labels.push(label.to_string());
                }
            }
            cursor = next;
            if cursor == 0 {
                break;
            }
        }
        Ok(labels)
    }

    // ── Graph administration reads ──────────────────────────────────────────

    pub(crate) fn graph_exists_impl(&self, name: &str) -> Result<bool, StorageError> {
        let mut conn = self.route_read()?;
        redis::cmd("SISMEMBER")
            .arg(self.key_graphs())
            .arg(name)
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    pub(crate) fn list_graphs_impl(&self) -> Result<Vec<String>, StorageError> {
        let mut conn = self.route_read()?;
        redis::cmd("SMEMBERS")
            .arg(self.key_graphs())
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    pub(crate) fn graph_stats_impl(&self, graph: &str) -> Result<GraphStats, StorageError> {
        self.require_graph(graph)?;
        let mut conn = self.route_read()?;
        let node_count: u64 = redis::cmd("SCARD")
            .arg(self.kp(key_nodes_set(graph)))
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        let edge_count: u64 = redis::cmd("SCARD")
            .arg(self.kp(key_edges_set(graph)))
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Count distinct labels and rel-types by scanning index keys.
        let label_pattern = format!("{}G:{{{graph}}}:label:*", self.key_prefix);
        let reltype_pattern = format!("{}G:{{{graph}}}:reltype:*", self.key_prefix);
        let label_count = scan_key_count(conn.as_mut(), &label_pattern)?;
        let rel_type_count = scan_key_count(conn.as_mut(), &reltype_pattern)?;
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
