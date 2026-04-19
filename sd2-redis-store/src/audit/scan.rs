//! Read-only scan helpers used by `audit_graph_metadata` to detect stale
//! Redis-only graph metadata before any repair lock is acquired.

use cypher_core::error::StorageError;
use redis::ConnectionLike;

use crate::keys::{
    key_edge_seq, key_edges_set, key_indexes, key_label, key_label_names, key_node_seq,
    key_nodes_set, key_propidx_manifest, key_propidx_prefix, key_reltype, key_reltype_names,
};
use crate::RedisGraphStore;

impl RedisGraphStore {
    pub(super) fn graph_registry_state_on(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
    ) -> Result<(bool, bool), StorageError> {
        let registered: bool = redis::cmd("SISMEMBER")
            .arg(self.key_graphs())
            .arg(graph)
            .query(conn)
            .map_err(crate::backend_storage_error)?;
        let deleting = self.graph_is_deleting_on(conn, graph)?;
        Ok((registered, deleting))
    }

    pub(super) fn read_set_members(
        &self,
        conn: &mut dyn ConnectionLike,
        key: &str,
    ) -> Result<Vec<String>, StorageError> {
        redis::cmd("SMEMBERS")
            .arg(key)
            .query(conn)
            .map_err(crate::backend_storage_error)
    }

    pub(super) fn collect_stale_labels_on(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        members: &[String],
    ) -> Result<Vec<String>, StorageError> {
        let mut stale = Vec::new();
        for label in members {
            let count: u64 = redis::cmd("SCARD")
                .arg(self.kp(key_label(graph, label)))
                .query(conn)
                .map_err(crate::backend_storage_error)?;
            if count == 0 {
                stale.push(label.clone());
            }
        }
        Ok(stale)
    }

    pub(super) fn collect_stale_reltypes_on(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        members: &[String],
    ) -> Result<Vec<String>, StorageError> {
        let mut stale = Vec::new();
        for reltype in members {
            let count: u64 = redis::cmd("SCARD")
                .arg(self.kp(key_reltype(graph, reltype)))
                .query(conn)
                .map_err(crate::backend_storage_error)?;
            if count == 0 {
                stale.push(reltype.clone());
            }
        }
        Ok(stale)
    }

    pub(super) fn collect_stale_propidx_on(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        members: &[String],
    ) -> Result<Vec<String>, StorageError> {
        let mut stale = Vec::new();
        for member in members {
            if !self.propidx_member_matches_graph(graph, member) {
                stale.push(member.clone());
                continue;
            }

            let exists: i64 = redis::cmd("EXISTS")
                .arg(member)
                .query(conn)
                .map_err(crate::backend_storage_error)?;
            if exists == 0 {
                stale.push(member.clone());
            }
        }
        Ok(stale)
    }

    pub(super) fn graph_cleanup_roots_absent_on(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
    ) -> Result<bool, StorageError> {
        for key in [
            self.kp(key_node_seq(graph)),
            self.kp(key_edge_seq(graph)),
            self.kp(key_nodes_set(graph)),
            self.kp(key_edges_set(graph)),
            self.kp(key_indexes(graph)),
            self.kp(key_label_names(graph)),
            self.kp(key_reltype_names(graph)),
            self.kp(key_propidx_manifest(graph)),
        ] {
            let exists: i64 = redis::cmd("EXISTS")
                .arg(&key)
                .query(conn)
                .map_err(crate::backend_storage_error)?;
            if exists > 0 {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn propidx_member_matches_graph(&self, graph: &str, member: &str) -> bool {
        member.starts_with(&self.kp(key_propidx_prefix(graph)))
    }
}
