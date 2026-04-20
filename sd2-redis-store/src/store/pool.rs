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

#[cfg(test)]
mod tests {
    //! Tests for pool metrics and routing helpers.
    //!
    //! [`RedisGraphStore::new_for_testing`] yields a store with a primary
    //! pool pointed at an unreachable URL. Construction succeeds (min_idle=0)
    //! but `pool.get()` always errors — that's exactly what we want to
    //! exercise the error branches of `conn`, `route_read`, and
    //! `route_read_with_pinning`.

    use super::*;
    use crate::connection::ReplicaPool;
    use cypher_core::storage::ReadConsistency;

    fn store() -> RedisGraphStore {
        RedisGraphStore::new_for_testing()
    }

    fn unwrap_err_conn(
        r: Result<crate::connection::RedisConn, StorageError>,
        ctx: &str,
    ) -> StorageError {
        match r {
            Err(e) => e,
            Ok(_) => panic!("{ctx}: expected Err, got Ok"),
        }
    }

    #[test]
    fn pool_stats_returns_snapshot() {
        let s = store();
        let stats = s.pool_stats();
        // The pool was built with min_idle=0; on a freshly constructed pool
        // that has not handed out any connections, idle_connections is 0
        // and active is 0. We just exercise the call and verify it returns
        // without panicking.
        let _ = (stats.size, stats.idle, stats.active);
    }

    #[test]
    fn conn_returns_backend_error_when_no_server() {
        let s = store();
        let err = unwrap_err_conn(s.conn(), "checkout against unreachable URL must fail");
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn route_read_falls_through_to_primary_when_no_replica_pool() {
        let s = store();
        // No replica pool configured — route_read goes straight to primary,
        // which fails because the URL is unreachable.
        let err = unwrap_err_conn(
            s.route_read(),
            "route_read with no replica must hit the primary and fail",
        );
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn route_read_with_pinning_force_primary_uses_primary_pool() {
        let mut s = store();
        // Even when a replica pool is configured, force_primary=true skips it
        // and routes to the primary (which fails with no live server).
        s.replica_pool = Some(
            ReplicaPool::new(&["redis://127.0.0.1:2"], 1, 1, 60, 0)
                .expect("replica pool construction should not require a live Redis"),
        );
        let err = unwrap_err_conn(
            s.route_read_with_pinning(true),
            "force_primary must bypass replica",
        );
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn route_read_with_pinning_uses_replica_when_available() {
        let mut s = store();
        s.replica_pool = Some(
            ReplicaPool::new(&["redis://127.0.0.1:2"], 1, 1, 60, 0)
                .expect("replica pool construction should not require a live Redis"),
        );
        // force_primary=false: replica pool is consulted, which also fails
        // because the URL is unreachable. Either way we want the Backend
        // error path covered.
        let err = unwrap_err_conn(
            s.route_read_with_pinning(false),
            "replica route must fail without a live replica",
        );
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn route_read_honors_session_fresh_primary_consistency() {
        let mut s = store();
        s.replica_pool = Some(ReplicaPool::new(&["redis://127.0.0.1:2"], 1, 1, 60, 0).unwrap());
        // Without SessionFreshPrimary, route_read uses the replica.
        let err = unwrap_err_conn(s.route_read(), "backend error expected");
        assert!(matches!(err, StorageError::Backend { .. }));

        // With SessionFreshPrimary, route_read pins to the primary even when
        // a replica pool exists.
        let pinned = s.with_read_consistency_mode(ReadConsistency::SessionFreshPrimary);
        let err = unwrap_err_conn(pinned.route_read(), "backend error expected");
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn route_read_for_graph_propagates_backend_error_when_pool_unreachable() {
        let s = store();
        // require_graph_on never runs because pool.get() fails first.
        let err = unwrap_err_conn(
            s.route_read_for_graph("g"),
            "graph-scoped read against unreachable URL must fail",
        );
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn spawn_pool_stats_reporter_runs_under_tokio_runtime() {
        // The reporter spawns a tokio task that polls every 15 s; we just
        // verify the call doesn't panic when invoked under a runtime.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("tokio runtime must build");
        rt.block_on(async {
            store().spawn_pool_stats_reporter();
        });
    }
}
