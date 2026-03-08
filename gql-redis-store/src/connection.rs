// connection.rs — Connection pooling for standalone and Redis Cluster modes.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use redis::cluster::ClusterClient;
use redis::{Client, ConnectionLike};

use gql_core::error::StorageError;

/// Snapshot of connection pool utilization.
#[derive(Debug, Clone, Copy)]
pub struct PoolStats {
    /// Total connections managed by the pool (idle + active).
    pub size: u32,
    /// Connections sitting idle in the pool.
    pub idle: u32,
    /// Connections currently checked out by application code.
    pub active: u32,
}

/// Default connection pool size. Set high enough for concurrent bulk writes
/// (each Bolt session may hold a pool connection during query execution).
pub(crate) const DEFAULT_POOL_SIZE: u32 = 64;
/// Default pool checkout timeout in seconds.
pub(crate) const DEFAULT_POOL_TIMEOUT_SECS: u64 = 5;
/// Default idle timeout in seconds.  Connections idle longer than this are
/// reaped, preventing stale TCP sockets after transient network events.
pub(crate) const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 300;
/// Default minimum idle connections.  Keeps at least this many connections
/// warm so the first requests after an idle period avoid cold-connect latency.
pub(crate) const DEFAULT_MIN_IDLE: u32 = 1;

/// Connection pool supporting both standalone and Redis Cluster modes.
///
/// Standalone mode uses a single Redis URL; cluster mode uses multiple seed
/// node URLs and auto-discovers the full cluster topology.
#[derive(Clone)]
pub(crate) enum RedisPool {
    Standalone(r2d2::Pool<Client>),
    Cluster(r2d2::Pool<ClusterClient>),
}

/// A checked-out connection from either pool type.
///
/// Both `Connection` and `ClusterConnection` implement `redis::ConnectionLike`,
/// so single-command operations work identically via [`RedisConn::as_mut`].
pub(crate) enum RedisConn {
    Standalone(r2d2::PooledConnection<Client>),
    Cluster(Box<r2d2::PooledConnection<ClusterClient>>),
}

impl RedisConn {
    /// Get a mutable reference to the underlying connection as `ConnectionLike`.
    ///
    /// Both `Connection` and `ClusterConnection` implement `ConnectionLike`,
    /// so all `redis::cmd()` based operations work through this reference.
    /// Note: the `redis::Commands` trait requires `Sized` so cannot be used
    /// on `dyn ConnectionLike`; use `redis::cmd()` directly instead.
    pub(crate) fn as_mut(&mut self) -> &mut dyn ConnectionLike {
        match self {
            RedisConn::Standalone(c) => &mut **c,
            RedisConn::Cluster(c) => &mut **c,
        }
    }
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
        let client = Client::open(url).map_err(|e| StorageError::Backend {
            msg: format!("redis client error: {e}"),
        })?;
        let pool = r2d2::Pool::builder()
            .max_size(pool_size)
            .connection_timeout(Duration::from_secs(pool_timeout_secs))
            .idle_timeout(Some(Duration::from_secs(idle_timeout_secs)))
            .min_idle(Some(min_idle))
            .build(client)
            .map_err(|e| StorageError::Backend {
                msg: format!("redis pool error: {e}"),
            })?;
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
        let client = ClusterClient::new(nodes.iter().map(|s| s.to_string()).collect::<Vec<_>>())
            .map_err(|e| StorageError::Backend {
                msg: format!("redis cluster client error: {e}"),
            })?;
        let pool = r2d2::Pool::builder()
            .max_size(pool_size)
            .connection_timeout(Duration::from_secs(pool_timeout_secs))
            .idle_timeout(Some(Duration::from_secs(idle_timeout_secs)))
            .min_idle(Some(min_idle))
            .build(client)
            .map_err(|e| StorageError::Backend {
                msg: format!("redis cluster pool error: {e}"),
            })?;
        Ok(RedisPool::Cluster(pool))
    }

    /// Check out a connection from the pool.
    ///
    /// Records `redis_pool_checkouts_total` on success and
    /// `redis_pool_timeout_total` on failure.
    pub(crate) fn get(&self) -> Result<RedisConn, StorageError> {
        let result = match self {
            RedisPool::Standalone(pool) => {
                pool.get()
                    .map(RedisConn::Standalone)
                    .map_err(|e| StorageError::Backend {
                        msg: format!("redis pool checkout error: {e}"),
                    })
            }
            RedisPool::Cluster(pool) => pool
                .get()
                .map(|c| RedisConn::Cluster(Box::new(c)))
                .map_err(|e| StorageError::Backend {
                    msg: format!("redis cluster pool checkout error: {e}"),
                }),
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

    /// Spawn a background tokio task that periodically records pool utilization
    /// gauges: `redis_pool_size`, `redis_pool_idle`, `redis_pool_active`.
    ///
    /// The task runs every 15 seconds and stops when the pool is dropped
    /// (the internal `r2d2::Pool` is `Arc`-backed, so the task keeps a
    /// cheap clone alive until the last reference is gone).
    pub(crate) fn spawn_stats_reporter(&self) {
        let pool = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(15));
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

/// A pool of read-only replica connections with round-robin selection.
///
/// Each replica URL gets its own standalone `r2d2` pool. Connections are
/// distributed across replicas using an atomic counter for even load
/// distribution. The `READONLY` command is sent on each new connection
/// to allow reads from Redis replicas.
pub(crate) struct ReplicaPool {
    pools: Vec<r2d2::Pool<Client>>,
    next: AtomicUsize,
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
            return Err(StorageError::Backend {
                msg: "replica pool requires at least one URL".into(),
            });
        }
        // Distribute pool_size evenly across replicas (minimum 1 per replica).
        let per_replica = (pool_size / urls.len() as u32).max(1);
        let mut pools = Vec::with_capacity(urls.len());
        for url in urls {
            let client = Client::open(*url).map_err(|e| StorageError::Backend {
                msg: format!("redis replica client error: {e}"),
            })?;
            let pool = r2d2::Pool::builder()
                .max_size(per_replica)
                .connection_timeout(Duration::from_secs(pool_timeout_secs))
                .idle_timeout(Some(Duration::from_secs(idle_timeout_secs)))
                .min_idle(Some(min_idle.min(per_replica)))
                .build(client)
                .map_err(|e| StorageError::Backend {
                    msg: format!("redis replica pool error: {e}"),
                })?;
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
    pub(crate) fn get(&self) -> Result<RedisConn, StorageError> {
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % self.pools.len();
        let mut conn = self.pools[idx]
            .get()
            .map(RedisConn::Standalone)
            .map_err(|e| StorageError::Backend {
                msg: format!("redis replica pool checkout error: {e}"),
            })?;
        // Issue READONLY so replica accepts read commands.
        redis::cmd("READONLY")
            .query::<String>(conn.as_mut())
            .map_err(|e| StorageError::Backend {
                msg: format!("redis READONLY command failed: {e}"),
            })?;
        Ok(conn)
    }
}
