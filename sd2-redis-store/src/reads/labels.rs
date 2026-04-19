// Label and relationship-type registry reads (active names with empty-set filtering).

use cypher_core::error::StorageError;

use crate::connection::RedisConn;
use crate::keys::{
    key_edge, key_edges_set, key_label, key_label_names, key_node, key_nodes_set, key_reltype,
    key_reltype_names,
};
use crate::RedisGraphStore;

use super::helpers::{smembers_or_scan, smembers_threshold};

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
}
