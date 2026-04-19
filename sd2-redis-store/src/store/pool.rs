// Pool metrics and low-level connection-routing helpers for [`RedisGraphStore`].

use cypher_core::error::StorageError;
use cypher_core::storage::ReadConsistency;

use crate::connection::{PoolStats, RedisConn};

use super::RedisGraphStore;

impl RedisGraphStore {
    // ── Pool metrics ────────────────────────────────────────────────────────

    /// Return a snapshot of current connection pool utilization.
    ///
    /// Callers (bolt, api) can integrate this with their metrics endpoints
    /// or health checks.
    pub fn pool_stats(&self) -> PoolStats {
        self.pool.stats()
    }

    /// Spawn a background tokio task that records pool utilization gauges
    /// every 15 seconds: `redis_pool_size`, `redis_pool_idle`,
    /// `redis_pool_active`.
    ///
    /// Call once after constructing the store (requires a running tokio
    /// runtime).
    pub fn spawn_pool_stats_reporter(&self) {
        self.pool.spawn_stats_reporter();
    }

    // ── Internal helpers (shared across modules) ────────────────────────────

    /// Check out a connection from the pool.
    pub(crate) fn conn(&self) -> Result<RedisConn, StorageError> {
        self.pool.get()
    }

    /// Check out a read-only connection, preferring replicas when available.
    ///
    /// If a replica pool is configured (`GQL_REDIS_REPLICA_URL`), returns a
    /// connection from the replica pool with round-robin selection. Otherwise
    /// falls back to the primary pool.
    ///
    /// The store's session-scoped `read_consistency` can force reads to the
    /// primary when a caller needs read-your-writes consistency.
    pub(crate) fn route_read(&self) -> Result<RedisConn, StorageError> {
        self.route_read_with_pinning(matches!(
            self.read_consistency,
            ReadConsistency::SessionFreshPrimary
        ))
    }

    /// Like [`route_read`] but allows forcing reads to the primary for
    /// read-after-write consistency.
    pub(crate) fn route_read_with_pinning(
        &self,
        force_primary: bool,
    ) -> Result<RedisConn, StorageError> {
        if !force_primary {
            if let Some(ref rp) = self.replica_pool {
                return rp.get();
            }
        }
        self.pool.get()
    }

    /// Route a graph-scoped read, retrying on the primary when a replica lags.
    ///
    /// Replica reads are allowed to return stale graph contents, but they must
    /// not surface a false `GraphNotFound` for a graph that is already
    /// registered on the primary. When a replica misses the graph registry,
    /// retry once against the primary before returning an error. A
    /// `SessionFreshPrimary` store view bypasses replica-first routing because
    /// `route_read()` already starts on the primary.
    pub(crate) fn route_read_for_graph(&self, graph: &str) -> Result<RedisConn, StorageError> {
        let force_primary = matches!(self.read_consistency, ReadConsistency::SessionFreshPrimary);

        let mut conn = self.route_read_with_pinning(force_primary)?;
        match self.require_graph_on(conn.as_mut(), graph) {
            Ok(()) => Ok(conn),
            Err(StorageError::GraphNotFound { .. })
                if self.replica_pool.is_some() && !force_primary =>
            {
                tracing::debug!(
                    graph,
                    "replica missed graph registration; retrying graph-scoped read on primary"
                );
                let mut primary = self.route_read_with_pinning(true)?;
                self.require_graph_on(primary.as_mut(), graph)?;
                Ok(primary)
            }
            Err(err) => Err(err),
        }
    }
}
