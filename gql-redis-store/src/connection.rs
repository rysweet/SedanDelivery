// connection.rs — Connection pooling for standalone and Redis Cluster modes.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use redis::cluster::{ClusterClient, ClusterClientBuilder, ClusterConnection, ClusterPipeline};
use redis::{
    Client, Connection, ConnectionLike, FromRedisValue, RedisError, RedisResult, ToRedisArgs,
};

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

/// Configurable stats reporting interval, read from `GQL_REDIS_STATS_INTERVAL_SECS`
/// env var (default 15, minimum 1, maximum 3600).
fn stats_interval_secs() -> u64 {
    static INTERVAL: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *INTERVAL.get_or_init(|| {
        gql_core::config::parse_env_range("GQL_REDIS_STATS_INTERVAL_SECS", 15u64, 1, 3600)
    })
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
/// Default Redis TCP connect timeout in milliseconds.
pub(crate) const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 1000;
/// Default Redis socket read timeout in milliseconds.
pub(crate) const DEFAULT_READ_TIMEOUT_MS: u64 = 5000;
/// Default Redis socket write timeout in milliseconds.
pub(crate) const DEFAULT_WRITE_TIMEOUT_MS: u64 = 5000;

#[derive(Debug, Clone, Copy)]
struct RedisSocketTimeouts {
    connect_timeout: Duration,
    read_timeout: Duration,
    write_timeout: Duration,
}

fn socket_connect_timeout_ms() -> u64 {
    static CONNECT_TIMEOUT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *CONNECT_TIMEOUT.get_or_init(|| {
        gql_core::config::parse_env_range(
            "GQL_REDIS_CONNECT_TIMEOUT_MS",
            DEFAULT_CONNECT_TIMEOUT_MS,
            1,
            300_000,
        )
    })
}

fn socket_read_timeout_ms() -> u64 {
    static READ_TIMEOUT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *READ_TIMEOUT.get_or_init(|| {
        gql_core::config::parse_env_range(
            "GQL_REDIS_READ_TIMEOUT_MS",
            DEFAULT_READ_TIMEOUT_MS,
            1,
            300_000,
        )
    })
}

fn socket_write_timeout_ms() -> u64 {
    static WRITE_TIMEOUT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *WRITE_TIMEOUT.get_or_init(|| {
        gql_core::config::parse_env_range(
            "GQL_REDIS_WRITE_TIMEOUT_MS",
            DEFAULT_WRITE_TIMEOUT_MS,
            1,
            300_000,
        )
    })
}

fn socket_timeouts() -> RedisSocketTimeouts {
    RedisSocketTimeouts {
        connect_timeout: Duration::from_millis(socket_connect_timeout_ms()),
        read_timeout: Duration::from_millis(socket_read_timeout_ms()),
        write_timeout: Duration::from_millis(socket_write_timeout_ms()),
    }
}

fn closed_connection_error() -> RedisError {
    RedisError::from(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
}

#[derive(Clone)]
pub(crate) struct TimeoutClientManager {
    client: Client,
    timeouts: RedisSocketTimeouts,
}

impl TimeoutClientManager {
    fn new(client: Client, timeouts: RedisSocketTimeouts) -> Self {
        Self { client, timeouts }
    }
}

impl r2d2::ManageConnection for TimeoutClientManager {
    type Connection = Connection;
    type Error = RedisError;

    fn connect(&self) -> Result<Self::Connection, Self::Error> {
        let conn = self
            .client
            .get_connection_with_timeout(self.timeouts.connect_timeout)?;
        conn.set_read_timeout(Some(self.timeouts.read_timeout))?;
        conn.set_write_timeout(Some(self.timeouts.write_timeout))?;
        Ok(conn)
    }

    fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), Self::Error> {
        if conn.check_connection() {
            Ok(())
        } else {
            Err(closed_connection_error())
        }
    }

    fn has_broken(&self, conn: &mut Self::Connection) -> bool {
        !conn.is_open()
    }
}

#[derive(Clone)]
pub(crate) struct TimeoutClusterClientManager {
    client: ClusterClient,
    timeouts: RedisSocketTimeouts,
}

impl TimeoutClusterClientManager {
    fn new(client: ClusterClient, timeouts: RedisSocketTimeouts) -> Self {
        Self { client, timeouts }
    }
}

impl r2d2::ManageConnection for TimeoutClusterClientManager {
    type Connection = ClusterConnection;
    type Error = RedisError;

    fn connect(&self) -> Result<Self::Connection, Self::Error> {
        let conn = self.client.get_connection()?;
        conn.set_read_timeout(Some(self.timeouts.read_timeout))?;
        conn.set_write_timeout(Some(self.timeouts.write_timeout))?;
        Ok(conn)
    }

    fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), Self::Error> {
        if conn.check_connection() {
            Ok(())
        } else {
            Err(closed_connection_error())
        }
    }

    fn has_broken(&self, conn: &mut Self::Connection) -> bool {
        !conn.is_open()
    }
}

/// Connection pool supporting both standalone and Redis Cluster modes.
///
/// Standalone mode uses a single Redis URL; cluster mode uses multiple seed
/// node URLs and auto-discovers the full cluster topology.
#[derive(Clone)]
pub(crate) enum RedisPool {
    Standalone(r2d2::Pool<TimeoutClientManager>),
    Cluster(r2d2::Pool<TimeoutClusterClientManager>),
}

/// A checked-out connection from either pool type.
///
/// Both `Connection` and `ClusterConnection` implement `redis::ConnectionLike`,
/// so single-command operations work identically via [`RedisConn::as_mut`].
pub(crate) enum RedisConn {
    Standalone(r2d2::PooledConnection<TimeoutClientManager>),
    Cluster(Box<r2d2::PooledConnection<TimeoutClusterClientManager>>),
}

/// Cluster-aware Redis pipeline wrapper.
///
/// `redis::Pipeline::query()` is invalid for `ClusterConnection`; cluster mode
/// must use `redis::cluster::cluster_pipe()` instead. This wrapper preserves the
/// current pipeline-building call pattern while dispatching execution to the
/// correct redis-rs pipeline type for the checked-out connection.
pub(crate) enum RedisPipeline {
    Standalone(redis::Pipeline),
    Cluster(ClusterPipeline),
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

    /// Create a pipeline builder that matches this connection's concrete type.
    pub(crate) fn pipeline(&self) -> RedisPipeline {
        match self {
            RedisConn::Standalone(_) => RedisPipeline::Standalone(redis::pipe()),
            RedisConn::Cluster(_) => RedisPipeline::Cluster(redis::cluster::cluster_pipe()),
        }
    }

    /// Return `true` if this is a standalone (non-cluster) connection.
    ///
    /// Used to emit a diagnostic warning when pipelines execute without the
    /// atomicity guarantee that was formerly provided by `.atomic()` (MULTI/EXEC).
    /// In cluster mode the absence of MULTI/EXEC is intentional and required;
    /// in standalone mode it represents a deliberate atomicity trade-off that
    /// should be visible in logs. See ADR-005.
    #[inline]
    pub(crate) fn is_standalone(&self) -> bool {
        matches!(self, RedisConn::Standalone(_))
    }
}

impl RedisPipeline {
    #[inline]
    pub(crate) fn cmd(&mut self, name: &str) -> &mut Self {
        match self {
            RedisPipeline::Standalone(pipe) => {
                pipe.cmd(name);
            }
            RedisPipeline::Cluster(pipe) => {
                pipe.cmd(name);
            }
        }
        self
    }

    #[inline]
    pub(crate) fn arg<T: ToRedisArgs>(&mut self, arg: T) -> &mut Self {
        match self {
            RedisPipeline::Standalone(pipe) => {
                pipe.arg(arg);
            }
            RedisPipeline::Cluster(pipe) => {
                pipe.arg(arg);
            }
        }
        self
    }

    #[inline]
    pub(crate) fn ignore(&mut self) -> &mut Self {
        match self {
            RedisPipeline::Standalone(pipe) => {
                pipe.ignore();
            }
            RedisPipeline::Cluster(pipe) => {
                pipe.ignore();
            }
        }
        self
    }

    #[inline]
    pub(crate) fn query<T: FromRedisValue>(&self, conn: &mut RedisConn) -> RedisResult<T> {
        match (self, conn) {
            (RedisPipeline::Standalone(pipe), RedisConn::Standalone(c)) => pipe.query(&mut **c),
            (RedisPipeline::Cluster(pipe), RedisConn::Cluster(c)) => pipe.query(c),
            _ => unreachable!("pipeline type must match connection type"),
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
}
