//! Lock-bound recheck helpers used during the repair pass of
//! `audit_graph_metadata`. These re-validate stale candidates while holding
//! the per-graph write lock and renew the lock periodically for large batches.

use cypher_core::error::StorageError;

use crate::keys::{key_label, key_reltype};
use crate::{LockedRedisConn, RedisGraphStore};

impl RedisGraphStore {
    pub(super) fn recheck_stale_labels_under_lock(
        &self,
        conn: &mut LockedRedisConn,
        graph: &str,
        candidates: &[String],
    ) -> Result<Vec<String>, StorageError> {
        let mut stale = Vec::new();
        for (index, label) in candidates.iter().enumerate() {
            if index > 0 && index % 100 == 0 {
                conn.renew_lock()?;
            }
            let count: u64 = redis::cmd("SCARD")
                .arg(self.kp(key_label(graph, label)))
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;
            if count == 0 {
                stale.push(label.clone());
            }
        }
        Ok(stale)
    }

    pub(super) fn recheck_stale_reltypes_under_lock(
        &self,
        conn: &mut LockedRedisConn,
        graph: &str,
        candidates: &[String],
    ) -> Result<Vec<String>, StorageError> {
        let mut stale = Vec::new();
        for (index, reltype) in candidates.iter().enumerate() {
            if index > 0 && index % 100 == 0 {
                conn.renew_lock()?;
            }
            let count: u64 = redis::cmd("SCARD")
                .arg(self.kp(key_reltype(graph, reltype)))
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;
            if count == 0 {
                stale.push(reltype.clone());
            }
        }
        Ok(stale)
    }

    pub(super) fn recheck_stale_propidx_under_lock(
        &self,
        conn: &mut LockedRedisConn,
        graph: &str,
        candidates: &[String],
    ) -> Result<Vec<String>, StorageError> {
        let mut stale = Vec::new();
        for (index, member) in candidates.iter().enumerate() {
            if index > 0 && index % 100 == 0 {
                conn.renew_lock()?;
            }
            if !self.propidx_member_matches_graph(graph, member) {
                stale.push(member.clone());
                continue;
            }
            let exists: i64 = redis::cmd("EXISTS")
                .arg(member)
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;
            if exists == 0 {
                stale.push(member.clone());
            }
        }
        Ok(stale)
    }
}
