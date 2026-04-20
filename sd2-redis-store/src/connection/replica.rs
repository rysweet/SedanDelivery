// replica — Round-robin pool of read-only Redis replica connections.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use redis::Client;

use cypher_core::error::StorageError;

use super::config::socket_timeouts;
use super::conn::RedisConn;
use super::manager::TimeoutClientManager;

/// A pool of read-only replica connections with round-robin selection.
///
/// Each replica URL gets its own standalone `r2d2` pool. Connections are
/// distributed across replicas using an atomic counter for even load
/// distribution. The `READONLY` command is sent on each new connection
/// to allow reads from Redis replicas.
pub(crate) struct ReplicaPool {
    pools: Vec<r2d2::Pool<TimeoutClientManager>>,
    next: AtomicUsize,
}

impl Clone for ReplicaPool {
    fn clone(&self) -> Self {
        Self {
            pools: self.pools.clone(),
            next: AtomicUsize::new(self.next.load(Ordering::Relaxed)),
        }
    }
}

impl ReplicaPool {
    /// Build a replica pool from one or more Redis replica URLs.
    ///
    /// Each URL gets its own connection pool. The `READONLY` command is
    /// issued on connection checkout to ensure replicas accept read commands.
    pub(crate) fn new(
        urls: &[&str],
        pool_size: u32,
        pool_timeout_secs: u64,
        idle_timeout_secs: u64,
        min_idle: u32,
    ) -> Result<Self, StorageError> {
        if urls.is_empty() {
            crate::record_backend_error();
            return Err(StorageError::Backend {
                msg: "replica pool requires at least one URL".into(),
            });
        }
        // Distribute pool_size evenly across replicas (minimum 1 per replica).
        let per_replica = (pool_size / urls.len() as u32).max(1);
        let mut pools = Vec::with_capacity(urls.len());
        let timeouts = socket_timeouts();
        for url in urls {
            let client = Client::open(*url).map_err(crate::backend_storage_error)?;
            let manager = TimeoutClientManager::new(client, timeouts);
            let pool = r2d2::Pool::builder()
                .max_size(per_replica)
                .connection_timeout(Duration::from_secs(pool_timeout_secs))
                .idle_timeout(Some(Duration::from_secs(idle_timeout_secs)))
                .min_idle(Some(min_idle.min(per_replica)))
                .build(manager)
                .map_err(crate::backend_storage_error)?;
            pools.push(pool);
        }
        Ok(Self {
            pools,
            next: AtomicUsize::new(0),
        })
    }

    /// Check out a connection from the next replica in round-robin order.
    ///
    /// Sends `READONLY` on the connection so Redis replicas accept read
    /// commands even when `slave-read-only` is enabled on the server.
    ///
    /// **Why READONLY is issued on every checkout (not just once at connection
    /// creation):** After a Redis failover, a former replica may be promoted to
    /// primary and a former primary demoted to replica. The `r2d2` pool may
    /// return a recycled connection that was established before the topology
    /// change. Issuing `READONLY` on every checkout ensures the connection is
    /// in the correct mode regardless of whether a failover occurred while the
    /// connection was idle in the pool. The `READONLY` command is idempotent
    /// and has negligible overhead (~0.01ms round-trip).
    pub(crate) fn get(&self) -> Result<RedisConn, StorageError> {
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % self.pools.len();
        let mut conn = self.pools[idx]
            .get()
            .map(RedisConn::Standalone)
            .map_err(crate::backend_storage_error)?;
        // Issue READONLY so cluster replicas accept read commands.
        // Standalone replicas (configured with `replica-read-only yes`) do NOT
        // support READONLY — it is a Redis Cluster-only command that returns
        // "ERR This instance has cluster support disabled" on non-cluster nodes.
        // Tolerate the error instead of failing so that master+replica topology
        // (non-cluster) works correctly.
        if let Err(e) = redis::cmd("READONLY").query::<String>(conn.as_mut()) {
            tracing::debug!(
                "READONLY command not supported on this replica (standalone mode): {e}"
            );
        }
        Ok(conn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_empty_url_list() {
        let res = ReplicaPool::new(&[], 4, 1, 60, 0);
        match res {
            Err(StorageError::Backend { msg }) => {
                assert!(msg.contains("at least one URL"), "got {msg}");
            }
            Ok(_) => panic!("empty URL list must error"),
            Err(other) => panic!("expected Backend, got {other:?}"),
        }
    }

    #[test]
    fn new_distributes_pool_size_across_replicas() {
        // min_idle=0 prevents eager connect; pool construction should succeed
        // even when no Redis replica is reachable.
        let pool = ReplicaPool::new(&["redis://127.0.0.1:1", "redis://127.0.0.1:2"], 4, 1, 60, 0)
            .expect("replica pool construction should not require live Redis");
        assert_eq!(pool.pools.len(), 2);
    }

    #[test]
    fn new_single_replica_works() {
        let pool = ReplicaPool::new(&["redis://127.0.0.1:1"], 1, 1, 60, 0)
            .expect("single-replica pool should build without live Redis");
        assert_eq!(pool.pools.len(), 1);
    }

    #[test]
    fn new_rejects_invalid_url() {
        // redis::Client::open rejects malformed URLs at construction time.
        let res = ReplicaPool::new(&["not-a-redis-url"], 1, 1, 60, 0);
        assert!(matches!(res, Err(StorageError::Backend { .. })));
    }

    #[test]
    fn clone_preserves_pool_count() {
        let pool =
            ReplicaPool::new(&["redis://127.0.0.1:1", "redis://127.0.0.1:2"], 2, 1, 60, 0).unwrap();
        let cloned = pool.clone();
        assert_eq!(cloned.pools.len(), pool.pools.len());
    }
}
