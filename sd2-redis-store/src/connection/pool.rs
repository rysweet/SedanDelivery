// pool — Primary [`RedisPool`] supporting standalone and Redis Cluster modes.

use std::time::Duration;

use redis::cluster::ClusterClientBuilder;
use redis::Client;

use cypher_core::error::StorageError;

use super::config::{socket_timeouts, stats_interval_secs, PoolStats};
use super::conn::RedisConn;
use super::manager::{TimeoutClientManager, TimeoutClusterClientManager};

/// Connection pool supporting both standalone and Redis Cluster modes.
///
/// Standalone mode uses a single Redis URL; cluster mode uses multiple seed
/// node URLs and auto-discovers the full cluster topology.
#[derive(Clone)]
pub(crate) enum RedisPool {
    Standalone(r2d2::Pool<TimeoutClientManager>),
    Cluster(r2d2::Pool<TimeoutClusterClientManager>),
}

impl RedisPool {
    /// Build a standalone connection pool from a Redis URL.
    pub(crate) fn standalone(
        url: &str,
        pool_size: u32,
        pool_timeout_secs: u64,
        idle_timeout_secs: u64,
        min_idle: u32,
    ) -> Result<Self, StorageError> {
        let client = Client::open(url).map_err(crate::backend_storage_error)?;
        let manager = TimeoutClientManager::new(client, socket_timeouts());
        let pool = r2d2::Pool::builder()
            .max_size(pool_size)
            .connection_timeout(Duration::from_secs(pool_timeout_secs))
            .idle_timeout(Some(Duration::from_secs(idle_timeout_secs)))
            .min_idle(Some(min_idle))
            .build(manager)
            .map_err(crate::backend_storage_error)?;
        Ok(RedisPool::Standalone(pool))
    }

    /// Build a cluster connection pool from seed node URLs.
    pub(crate) fn cluster(
        nodes: &[&str],
        pool_size: u32,
        pool_timeout_secs: u64,
        idle_timeout_secs: u64,
        min_idle: u32,
    ) -> Result<Self, StorageError> {
        let timeouts = socket_timeouts();
        let client = ClusterClientBuilder::new(nodes.iter().copied())
            .connection_timeout(timeouts.connect_timeout)
            .response_timeout(timeouts.read_timeout)
            .build()
            .map_err(crate::backend_storage_error)?;
        let manager = TimeoutClusterClientManager::new(client, timeouts);
        let pool = r2d2::Pool::builder()
            .max_size(pool_size)
            .connection_timeout(Duration::from_secs(pool_timeout_secs))
            .idle_timeout(Some(Duration::from_secs(idle_timeout_secs)))
            .min_idle(Some(min_idle))
            .build(manager)
            .map_err(crate::backend_storage_error)?;
        Ok(RedisPool::Cluster(pool))
    }

    /// Check out a connection from the pool.
    ///
    /// Records `redis_pool_checkouts_total` on success and
    /// `redis_pool_timeout_total` on failure.
    pub(crate) fn get(&self) -> Result<RedisConn, StorageError> {
        let result = match self {
            RedisPool::Standalone(pool) => pool
                .get()
                .map(RedisConn::Standalone)
                .map_err(crate::backend_storage_error),
            RedisPool::Cluster(pool) => pool
                .get()
                .map(|c| RedisConn::Cluster(Box::new(c)))
                .map_err(crate::backend_storage_error),
        };
        match &result {
            Ok(_) => metrics::counter!("redis_pool_checkouts_total").increment(1),
            Err(_) => metrics::counter!("redis_pool_timeout_total").increment(1),
        }
        result
    }

    /// Return a snapshot of current pool utilization.
    pub(crate) fn stats(&self) -> PoolStats {
        let s = match self {
            RedisPool::Standalone(pool) => pool.state(),
            RedisPool::Cluster(pool) => pool.state(),
        };
        PoolStats {
            size: s.connections,
            idle: s.idle_connections,
            active: s.connections.saturating_sub(s.idle_connections),
        }
    }

    /// Return `true` if this pool is in Redis Cluster mode.
    ///
    /// Test-only helper used to validate the pool variant without checking out
    /// a live connection.
    ///
    /// # Warning
    ///
    /// Do **not** use `is_cluster()` to gate `MULTI`/`EXEC` (`.atomic()`)
    /// in pipelines.  The correct approach is to never use `.atomic()` in
    /// any pipeline that touches per-graph keys: all such keys share the
    /// `{graphname}` hash tag and are therefore co-located on a single cluster
    /// slot, making non-atomic pipelining safe in both standalone and cluster
    /// modes.  Branching on `is_cluster()` would re-introduce MULTI/EXEC in
    /// standalone deployments and create a correctness gap between the two
    /// code paths.  See ADR-005 for the full rationale.
    #[cfg(test)]
    pub(crate) fn is_cluster(&self) -> bool {
        matches!(self, RedisPool::Cluster(_))
    }

    /// Spawn a background tokio task that periodically records pool utilization
    /// gauges: `redis_pool_size`, `redis_pool_idle`, `redis_pool_active`.
    ///
    /// The reporting interval is configurable via `GQL_REDIS_STATS_INTERVAL_SECS`
    /// (default 15, minimum 1, maximum 3600).
    ///
    /// **Lifetime**: This task runs for the entire process lifetime. The internal
    /// `r2d2::Pool` is `Arc`-backed, so the task keeps a cheap clone alive.
    /// There is no graceful shutdown mechanism; the task terminates when the
    /// tokio runtime is dropped (i.e., at process exit). This is intentional:
    /// pool metrics are only useful while the process is running, and adding a
    /// cancellation channel would require propagating a shutdown signal through
    /// the `RedisGraphStore` constructor, which is not warranted for a
    /// lightweight periodic gauge reporter.
    pub(crate) fn spawn_stats_reporter(&self) {
        let pool = self.clone();
        let interval_secs = stats_interval_secs();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            loop {
                interval.tick().await;
                let stats = pool.stats();
                metrics::gauge!("redis_pool_size").set(f64::from(stats.size));
                metrics::gauge!("redis_pool_idle").set(f64::from(stats.idle));
                metrics::gauge!("redis_pool_active").set(f64::from(stats.active));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── RedisPool::is_cluster ────────────────────────────────────────────────

    /// A standalone pool constructed from a Redis URL must NOT report itself
    /// as a cluster pool.
    ///
    #[test]
    fn standalone_pool_not_cluster() {
        // Build a standalone pool. This does NOT connect to Redis — r2d2 with
        // a URL-only Client does not open a connection until the first checkout.
        // We only need the pool type discrimination, not a live connection.
        let pool = RedisPool::standalone(
            "redis://127.0.0.1:6379",
            1,  // pool_size
            1,  // pool_timeout_secs
            60, // idle_timeout_secs
            0,  // min_idle
        )
        .expect("standalone pool construction should not require a live Redis server");
        assert!(
            !pool.is_cluster(),
            "standalone pool must return false for is_cluster()"
        );
    }

    /// A pool constructed via `RedisPool::cluster()` must report itself as a
    /// cluster pool.
    ///
    /// Note: ClusterClient construction succeeds without a live cluster.
    #[test]
    fn cluster_pool_is_cluster() {
        let pool = RedisPool::cluster(
            &["redis://127.0.0.1:7000"],
            1,  // pool_size
            1,  // pool_timeout_secs
            60, // idle_timeout_secs
            0,  // min_idle
        )
        .expect("cluster pool construction should not require a live cluster");
        assert!(
            pool.is_cluster(),
            "cluster pool must return true for is_cluster()"
        );
    }

    // ── Stats + clone + get-error paths (no live Redis) ────────────────

    #[test]
    fn standalone_stats_reports_zero_when_idle() {
        let pool = RedisPool::standalone("redis://127.0.0.1:1", 4, 1, 60, 0).unwrap();
        let s = pool.stats();
        // No connections have been checked out.
        assert_eq!(s.active, 0);
        assert!(s.size >= s.active);
        assert!(s.idle <= s.size);
    }

    #[test]
    fn cluster_stats_reports_zero_when_idle() {
        let pool = RedisPool::cluster(&["redis://127.0.0.1:7000"], 4, 1, 60, 0).unwrap();
        let s = pool.stats();
        assert_eq!(s.active, 0);
    }

    #[test]
    fn pool_clone_preserves_kind() {
        let pool = RedisPool::standalone("redis://127.0.0.1:1", 1, 1, 60, 0).unwrap();
        let cloned = pool.clone();
        assert_eq!(pool.is_cluster(), cloned.is_cluster());
    }

    #[test]
    fn standalone_get_returns_backend_error_when_no_server() {
        // min_idle=0 makes construction succeed; the first get() must attempt
        // a real TCP connect which fails fast against the bogus port, yielding
        // a StorageError::Backend (not a panic).
        let pool = RedisPool::standalone("redis://127.0.0.1:1", 1, 1, 60, 0).unwrap();
        let res = pool.get();
        assert!(res.is_err(), "expected get() to fail without live Redis");
    }
}
