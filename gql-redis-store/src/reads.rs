// reads.rs — All GraphStore read method implementations for RedisGraphStore.

use gql_core::error::StorageError;
use gql_core::serde_value::json_to_props;
use gql_core::storage::{AdjEntry, EdgeData, EdgeId, GraphStats, NodeData, NodeId, RangeQuery};

use crate::connection::RedisConn;
use crate::keys::{
    key_edge, key_edges_set, key_in, key_label, key_label_names, key_node, key_nodes_set, key_out,
    key_propidx, key_reltype, key_reltype_names, value_to_index_key, IDX_SEP,
};
use crate::range::range_matches;
use crate::{LockedRedisConn, RedisGraphStore};

/// Default member count threshold above which SSCAN is used instead of SMEMBERS.
/// Sets with fewer members than this use the faster SMEMBERS. Configurable via
/// `GQL_REDIS_SMEMBERS_THRESHOLD` environment variable.
pub(crate) fn smembers_threshold() -> u64 {
    static THRESHOLD: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *THRESHOLD.get_or_init(|| {
        gql_core::config::parse_env_min("GQL_REDIS_SMEMBERS_THRESHOLD", 10_000u64, 1)
    })
}

/// SSCAN page size for large-set iteration.
const SSCAN_COUNT: u64 = 1000;

/// Read all members of a Redis set, using SMEMBERS for small sets and SSCAN
/// for large ones to avoid blocking the Redis shard.
///
/// Checks `SCARD` first; if the set has more members than `threshold`, iterates
/// with SSCAN using COUNT 1000. Otherwise uses a single SMEMBERS call.
pub(crate) fn smembers_or_scan(
    conn: &mut dyn redis::ConnectionLike,
    key: &str,
    threshold: u64,
) -> Result<Vec<String>, StorageError> {
    let card: u64 = redis::cmd("SCARD")
        .arg(key)
        .query(conn)
        .map_err(crate::backend_storage_error)?;

    if card <= threshold {
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(key)
            .query(conn)
            .map_err(crate::backend_storage_error)?;
        return Ok(members);
    }

    // Large set: iterate with SSCAN to avoid blocking.
    let mut cursor: u64 = 0;
    let mut members = Vec::with_capacity(card as usize);
    loop {
        let (next_cursor, batch): (u64, Vec<String>) = redis::cmd("SSCAN")
            .arg(key)
            .arg(cursor)
            .arg("COUNT")
            .arg(SSCAN_COUNT)
            .query(conn)
            .map_err(crate::backend_storage_error)?;
        members.extend(batch);
        cursor = next_cursor;
        if cursor == 0 {
            break;
        }
    }
    Ok(members)
}

fn sscan_all_members(
    conn: &mut dyn redis::ConnectionLike,
    key: &str,
    capacity: usize,
) -> Result<Vec<String>, StorageError> {
    let mut cursor: u64 = 0;
    let mut members = Vec::with_capacity(capacity);
    loop {
        let (next_cursor, batch): (u64, Vec<String>) = redis::cmd("SSCAN")
            .arg(key)
            .arg(cursor)
            .arg("COUNT")
            .arg(SSCAN_COUNT)
            .query(conn)
            .map_err(crate::backend_storage_error)?;
        members.extend(batch);
        cursor = next_cursor;
        if cursor == 0 {
            break;
        }
    }
    Ok(members)
}

/// Visit set members in bounded pages while renewing the caller's write lock
/// between SSCAN batches.
pub(crate) fn for_each_set_page_locked<F>(
    conn: &mut LockedRedisConn,
    key: &str,
    threshold: u64,
    mut visit: F,
) -> Result<(), StorageError>
where
    F: FnMut(&mut LockedRedisConn, Vec<String>) -> Result<(), StorageError>,
{
    let card: u64 = redis::cmd("SCARD")
        .arg(key)
        .query(conn.as_mut())
        .map_err(crate::backend_storage_error)?;

    if card <= threshold {
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(key)
            .query(conn.as_mut())
            .map_err(crate::backend_storage_error)?;
        return visit(conn, members);
    }

    let mut cursor: u64 = 0;
    loop {
        conn.renew_lock()?;
        let (next_cursor, batch): (u64, Vec<String>) = redis::cmd("SSCAN")
            .arg(key)
            .arg(cursor)
            .arg("COUNT")
            .arg(SSCAN_COUNT)
            .query(conn.as_mut())
            .map_err(crate::backend_storage_error)?;
        visit(conn, batch)?;
        cursor = next_cursor;
        if cursor == 0 {
            break;
        }
    }
    Ok(())
}

impl RedisGraphStore {
    pub(crate) fn active_label_names(
        &self,
        conn: &mut RedisConn,
        graph: &str,
    ) -> Result<Vec<String>, StorageError> {
        let registry_key = self.kp(key_label_names(graph));
        let labels: Vec<String> = redis::cmd("SMEMBERS")
            .arg(&registry_key)
            .query(conn.as_mut())
            .map_err(crate::backend_storage_error)?;
        if !labels.is_empty() {
            let mut pipe = conn.pipeline();
            for label in &labels {
                pipe.cmd("SCARD").arg(self.kp(key_label(graph, label)));
            }
            let counts: Vec<u64> = pipe.query(conn).map_err(crate::backend_storage_error)?;
            let mut active: Vec<String> = labels
                .into_iter()
                .zip(counts)
                .filter_map(|(label, count)| (count > 0).then_some(label))
                .collect();
            active.sort_unstable();
            if !active.is_empty() {
                return Ok(active);
            }
        }

        let node_ids = smembers_or_scan(
            conn.as_mut(),
            &self.kp(key_nodes_set(graph)),
            smembers_threshold(),
        )?;
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut discovered = std::collections::HashSet::new();
        for chunk in node_ids.chunks(500) {
            let mut pipe = conn.pipeline();
            for nid_str in chunk {
                if let Ok(nid) = nid_str.parse::<u64>() {
                    pipe.cmd("HGET")
                        .arg(self.kp(key_node(graph, nid)))
                        .arg("labels");
                }
            }
            let results: Vec<Option<String>> =
                pipe.query(conn).map_err(crate::backend_storage_error)?;
            for json in results.into_iter().flatten() {
                let parsed: Vec<String> =
                    serde_json::from_str(&json).map_err(|e| StorageError::Serialization {
                        msg: format!("labels registry fallback: {e}"),
                    })?;
                discovered.extend(parsed);
            }
        }

        let mut labels: Vec<String> = discovered.into_iter().collect();
        labels.sort_unstable();
        Ok(labels)
    }

    pub(crate) fn active_reltype_names(
        &self,
        conn: &mut RedisConn,
        graph: &str,
    ) -> Result<Vec<String>, StorageError> {
        let registry_key = self.kp(key_reltype_names(graph));
        let reltypes: Vec<String> = redis::cmd("SMEMBERS")
            .arg(&registry_key)
            .query(conn.as_mut())
            .map_err(crate::backend_storage_error)?;
        if !reltypes.is_empty() {
            let mut pipe = conn.pipeline();
            for reltype in &reltypes {
                pipe.cmd("SCARD").arg(self.kp(key_reltype(graph, reltype)));
            }
            let counts: Vec<u64> = pipe.query(conn).map_err(crate::backend_storage_error)?;
            let mut active: Vec<String> = reltypes
                .into_iter()
                .zip(counts)
                .filter_map(|(reltype, count)| (count > 0).then_some(reltype))
                .collect();
            active.sort_unstable();
            if !active.is_empty() {
                return Ok(active);
            }
        }

        let edge_ids = smembers_or_scan(
            conn.as_mut(),
            &self.kp(key_edges_set(graph)),
            smembers_threshold(),
        )?;
        if edge_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut discovered = std::collections::HashSet::new();
        for chunk in edge_ids.chunks(500) {
            let mut pipe = conn.pipeline();
            for eid_str in chunk {
                if let Ok(eid) = eid_str.parse::<u64>() {
                    pipe.cmd("HGET")
                        .arg(self.kp(key_edge(graph, eid)))
                        .arg("rel_type");
                }
            }
            let results: Vec<Option<String>> =
                pipe.query(conn).map_err(crate::backend_storage_error)?;
            discovered.extend(results.into_iter().flatten());
        }

        let mut reltypes: Vec<String> = discovered.into_iter().collect();
        reltypes.sort_unstable();
        Ok(reltypes)
    }

    // ── Single-item reads ────────────────────────────────────────────────────

    pub(crate) fn get_node_impl(
        &self,
        graph: &str,
        id: NodeId,
    ) -> Result<Option<NodeData>, StorageError> {
        let mut conn = self.route_read_for_graph(graph)?;
        self.read_node(conn.as_mut(), graph, id.0)
    }

    pub(crate) fn get_edge_impl(
        &self,
        graph: &str,
        id: EdgeId,
    ) -> Result<Option<EdgeData>, StorageError> {
        let mut conn = self.route_read_for_graph(graph)?;
        self.read_edge(conn.as_mut(), graph, id.0)
    }

    // ── Batch reads ──────────────────────────────────────────────────────────

    pub(crate) fn get_nodes_batch_impl(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        let mut conn = self.route_read_for_graph(graph)?;
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let mut results = Vec::with_capacity(ids.len());
        // Pipeline all HMGET commands.
        let mut pipe = conn.pipeline();
        for id in ids {
            pipe.cmd("HMGET")
                .arg(self.kp(key_node(graph, id.0)))
                .arg("labels")
                .arg("props");
        }
        let raw: Vec<Vec<Option<String>>> = pipe
            .query(&mut conn)
            .map_err(crate::backend_storage_error)?;
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
        let mut conn = self.route_read_for_graph(graph)?;
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let mut pipe = conn.pipeline();
        for id in ids {
            pipe.cmd("HMGET")
                .arg(self.kp(key_edge(graph, id.0)))
                .arg("src")
                .arg("dst")
                .arg("rel_type")
                .arg("props");
        }
        let raw: Vec<Vec<Option<String>>> = pipe
            .query(&mut conn)
            .map_err(crate::backend_storage_error)?;
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
        let mut conn = self.route_read_for_graph(graph)?;
        let members = smembers_or_scan(
            conn.as_mut(),
            &self.kp(key_label(graph, label)),
            smembers_threshold(),
        )?;
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
        let mut conn = self.route_read_for_graph(graph)?;

        // Fast path: use property index for exact-match queries when an index exists.
        if let RangeQuery::Eq(val) = &range {
            let indexes = self.active_indexes(conn.as_mut(), graph)?;
            if indexes.contains(&format!("{label}{IDX_SEP}{property}")) {
                let vkey = value_to_index_key(val);
                let members = smembers_or_scan(
                    conn.as_mut(),
                    &self.kp(key_propidx(graph, label, property, &vkey)),
                    smembers_threshold(),
                )?;
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
        let mut conn = self.route_read_for_graph(graph)?;
        let members = smembers_or_scan(
            conn.as_mut(),
            &self.kp(key_reltype(graph, rel_type)),
            smembers_threshold(),
        )?;
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

    // ── Graph administration reads ──────────────────────────────────────────

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
