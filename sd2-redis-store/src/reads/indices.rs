// Label/type and property index reads.

use cypher_core::error::StorageError;
use cypher_core::serde_value::json_to_props;
use cypher_core::storage::{EdgeId, NodeId, RangeQuery};

use crate::keys::{key_label, key_node, key_propidx, key_reltype, value_to_index_key, IDX_SEP};
use crate::RedisGraphStore;

use super::helpers::{smembers_or_scan, smembers_threshold};

impl RedisGraphStore {
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

        // No declared property index — do a label scan + property filter in Redis.
        // This is slower than a property index but avoids the executor's fallback
        // which creates a cartesian product when used inside UNWIND + Apply.
        let label_members = smembers_or_scan(
            conn.as_mut(),
            &self.kp(key_label(graph, label)),
            smembers_threshold(),
        )?;

        // Parse member strings to u64 IDs, skip invalid ones.
        let node_ids: Vec<u64> = label_members
            .iter()
            .filter_map(|s| s.parse::<u64>().ok())
            .collect();

        // Pipeline HGET calls for the target property on all label members.
        // Use `conn.pipeline()` (not `redis::pipe()`) so that cluster
        // connections receive a `cluster_pipe()` capable of handling MOVED/ASK
        // redirects. All keys here share the `{graph}` hash tag and route to
        // the same shard, but the cluster pipeline wrapper is still required;
        // a standalone `redis::pipe()` against a `ClusterConnection` silently
        // returns no results, which manifested as missing label+property
        // matches in cluster mode (see #1090 follow-up: empty UNWIND-driven
        // MATCH results, missing edge-source/target nodes).
        let mut pipe = conn.pipeline();
        for &nid in &node_ids {
            pipe.cmd("HGET")
                .arg(self.kp(key_node(graph, nid)))
                .arg("props");
        }
        // Propagate pipeline errors instead of silently coercing to empty
        // results. The previous `unwrap_or_else(|_| vec![None; ...])` was
        // what allowed the cluster pipeline mismatch (#1276) to silently
        // return zero matches across the entire fleet for ~5 months.
        let props_results: Vec<Option<String>> = if node_ids.is_empty() {
            Vec::new()
        } else {
            pipe.query(&mut conn).map_err(|e| StorageError::Backend {
                msg: format!("pipelined HGET for label+property scan failed: {e}"),
            })?
        };

        let mut matched = Vec::new();
        for (&node_id, props_json) in node_ids.iter().zip(props_results.iter()) {
            if let Some(json) = props_json {
                if let Ok(props) = json_to_props(json) {
                    if let Some(val) = props.get(property) {
                        let matches = match &range {
                            RangeQuery::Eq(expected) => val == *expected,
                            _ => true, // Non-eq ranges: include all and let executor filter
                        };
                        if matches {
                            matched.push(NodeId(node_id));
                        }
                    }
                }
            }
        }
        Ok(matched)
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
}
