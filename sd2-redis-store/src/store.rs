// `RedisGraphStore` — the central store handle.
//
// Owns the primary and replica connection pools, the configured key prefix,
// and the session-scoped read-consistency contract. Exposes constructors,
// pool metrics, low-level connection helpers used by every other module in
// this crate, and the closure-based write-lock orchestration on top of which
// all mutating operations are layered.

use std::env;
use std::time::Instant;

use cypher_core::error::StorageError;
use cypher_core::storage::ReadConsistency;
use redis::ConnectionLike;

use crate::backend_storage_error;
use crate::connection::{
    PoolStats, RedisConn, RedisPool, ReplicaPool, DEFAULT_IDLE_TIMEOUT_SECS, DEFAULT_MIN_IDLE,
    DEFAULT_POOL_SIZE, DEFAULT_POOL_TIMEOUT_SECS,
};
use crate::error_metrics::{
    record_write_lock_backend_failure, REDIS_WRITE_LOCK_ACQUIRE_ATTEMPTS_TOTAL,
    REDIS_WRITE_LOCK_ACQUIRE_CONTENTION_TOTAL, REDIS_WRITE_LOCK_ACQUIRE_SUCCESSES_TOTAL,
    REDIS_WRITE_LOCK_ACQUIRE_WAIT_SECONDS, REDIS_WRITE_LOCK_CONTENTION_FAILURES_TOTAL,
};
use crate::keys::{key_graph_deleting, key_graph_lock, KEY_GRAPHS};
use crate::lock;
use crate::locked_conn::{map_lock_release_result, LockedRedisConn};

/// A [`GraphStore`] implementation backed by a Redis/Valkey server or cluster.
///
/// Connects to a standalone Redis/Valkey instance, Azure Cache for Redis,
/// or a Redis Cluster via multiple seed-node URLs.
///
/// Uses an `r2d2` connection pool for concurrent access from multiple threads
/// (e.g. multiple Bolt sessions sharing a single `RedisGraphStore`).
///
/// Connection configuration:
/// - `GQL_REDIS_CLUSTER_NODES` — comma-separated seed node URLs for cluster mode
/// - `GQL_REDIS_URL` / `REDIS_URL` — standalone URL (required if not using cluster mode)
///
/// Pool configuration:
/// - `GQL_REDIS_POOL_SIZE` — max connections (default 64)
/// - `GQL_REDIS_POOL_TIMEOUT_SECS` — checkout timeout (default 5)
/// - `GQL_REDIS_IDLE_TIMEOUT_SECS` — idle connection reap timeout (default 300)
/// - `GQL_REDIS_MIN_IDLE` — minimum warm connections kept in pool (default 1)
/// - `GQL_REDIS_CONNECT_TIMEOUT_MS` — Redis TCP connect timeout (default 1000)
/// - `GQL_REDIS_READ_TIMEOUT_MS` — Redis socket read timeout (default 5000)
/// - `GQL_REDIS_WRITE_TIMEOUT_MS` — Redis socket write timeout (default 5000)
/// - `GQL_REDIS_MAX_BATCH_SIZE` — logical sub-batch size for large batch node writes (default 5000)
/// - `GQL_REDIS_BATCH_PIPELINE_CHUNK_SIZE` — items per batch-write pipeline chunk (default 250)
/// - `GQL_REDIS_MAX_DETACH_DELETE_EDGES` — refuse pathological supernode detach deletes (default 10000)
///
/// Read-replica configuration:
/// - `GQL_REDIS_REPLICA_URL` — comma-separated replica URLs for read routing
#[derive(Clone)]
pub struct RedisGraphStore {
    pool: RedisPool,
    /// Optional read-replica pool. When present, `route_read()` returns
    /// connections from replicas; when absent, falls back to the primary pool.
    pub(crate) replica_pool: Option<ReplicaPool>,
    /// Optional key prefix for multi-tenant isolation.
    /// Set via `GQL_REDIS_KEY_PREFIX` environment variable (default empty).
    key_prefix: String,
    /// Session-scoped read-routing contract for this store view.
    read_consistency: ReadConsistency,
}

impl RedisGraphStore {
    /// Create a store from environment variables.
    ///
    /// Cluster mode is selected when `GQL_REDIS_CLUSTER_NODES` is set (comma-separated
    /// seed-node URLs).  Otherwise uses standalone mode from `GQL_REDIS_URL` or
    /// `REDIS_URL`.  Returns an error if neither is set.
    ///
    /// The key prefix is read from `GQL_REDIS_KEY_PREFIX` (default empty string).
    /// Set a prefix to isolate multiple deployments sharing a single Redis instance.
    ///
    /// Pool size and timeout are read from `GQL_REDIS_POOL_SIZE` (default 64)
    /// and `GQL_REDIS_POOL_TIMEOUT_SECS` (default 5).
    /// Idle timeout and min-idle are read from `GQL_REDIS_IDLE_TIMEOUT_SECS` (default 300)
    /// and `GQL_REDIS_MIN_IDLE` (default 1).
    /// Socket-level Redis timeouts are read from `GQL_REDIS_CONNECT_TIMEOUT_MS`
    /// (default 1000), `GQL_REDIS_READ_TIMEOUT_MS` (default 5000), and
    /// `GQL_REDIS_WRITE_TIMEOUT_MS` (default 5000).
    /// Large batch and detach-delete guards are controlled by
    /// `GQL_REDIS_MAX_BATCH_SIZE`, `GQL_REDIS_BATCH_PIPELINE_CHUNK_SIZE`, and
    /// `GQL_REDIS_MAX_DETACH_DELETE_EDGES`. `batch_create_nodes` and
    /// `batch_create_edges` stream large requests through smaller internal
    /// batches under one graph write lock instead of failing the whole request
    /// once it crosses the pipeline-sized threshold.
    pub fn from_env() -> Result<Self, StorageError> {
        let prefix = match env::var("GQL_REDIS_KEY_PREFIX") {
            Ok(p) => {
                tracing::info!(prefix = %p, "GQL_REDIS_KEY_PREFIX set");
                p
            }
            Err(_) => {
                tracing::info!("GQL_REDIS_KEY_PREFIX not set, using empty prefix");
                String::new()
            }
        };
        let pool_size: u32 =
            cypher_core::config::parse_env_range("GQL_REDIS_POOL_SIZE", DEFAULT_POOL_SIZE, 1, 1024);
        let pool_timeout_secs: u64 = cypher_core::config::parse_env_range(
            "GQL_REDIS_POOL_TIMEOUT_SECS",
            DEFAULT_POOL_TIMEOUT_SECS,
            1,
            300,
        );
        let idle_timeout_secs: u64 = cypher_core::config::parse_env_range(
            "GQL_REDIS_IDLE_TIMEOUT_SECS",
            DEFAULT_IDLE_TIMEOUT_SECS,
            0,
            86400,
        );
        let min_idle: u32 = cypher_core::config::parse_env_range(
            "GQL_REDIS_MIN_IDLE",
            DEFAULT_MIN_IDLE,
            0,
            pool_size,
        );

        // Build optional replica pool from GQL_REDIS_REPLICA_URL (comma-separated).
        let replica_pool = if let Ok(replica_str) = env::var("GQL_REDIS_REPLICA_URL") {
            let urls: Vec<&str> = replica_str.split(',').map(|s| s.trim()).collect();
            if urls.is_empty() || urls.iter().all(|u| u.is_empty()) {
                None
            } else {
                let valid_urls: Vec<&str> = urls.into_iter().filter(|u| !u.is_empty()).collect();
                match ReplicaPool::new(
                    &valid_urls,
                    pool_size,
                    pool_timeout_secs,
                    idle_timeout_secs,
                    min_idle,
                ) {
                    Ok(rp) => {
                        tracing::info!(
                            replica_count = valid_urls.len(),
                            "read-replica pool initialized"
                        );
                        Some(rp)
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "failed to initialize replica pool, reads will use primary"
                        );
                        None
                    }
                }
            }
        } else {
            None
        };

        // Cluster mode: GQL_REDIS_CLUSTER_NODES takes priority.
        if let Ok(nodes_str) = env::var("GQL_REDIS_CLUSTER_NODES") {
            let nodes: Vec<&str> = nodes_str.split(',').map(|s| s.trim()).collect();
            let mut store = Self::from_cluster_nodes(
                &nodes,
                &prefix,
                pool_size,
                pool_timeout_secs,
                idle_timeout_secs,
                min_idle,
            )?;
            store.replica_pool = replica_pool;
            return Ok(store);
        }

        // Standalone mode — require explicit URL.
        let url = env::var("GQL_REDIS_URL")
            .or_else(|_| env::var("REDIS_URL"))
            .map_err(|_| StorageError::Configuration {
                msg: "GQL_REDIS_URL or REDIS_URL must be set (no silent localhost fallback)".into(),
            })?;
        let mut store = Self::from_url_with_options(
            &url,
            &prefix,
            pool_size,
            pool_timeout_secs,
            idle_timeout_secs,
            min_idle,
        )?;
        store.replica_pool = replica_pool;
        Ok(store)
    }

    pub(crate) fn with_read_consistency_mode(mut self, read_consistency: ReadConsistency) -> Self {
        self.read_consistency = read_consistency;
        self
    }

    /// Create a store connecting to an explicit Redis URL with an optional key prefix.
    pub fn from_url_with_prefix(url: &str, key_prefix: &str) -> Result<Self, StorageError> {
        Self::from_url_with_options(
            url,
            key_prefix,
            DEFAULT_POOL_SIZE,
            DEFAULT_POOL_TIMEOUT_SECS,
            DEFAULT_IDLE_TIMEOUT_SECS,
            DEFAULT_MIN_IDLE,
        )
    }

    /// Create a store with full configuration control (standalone mode).
    pub fn from_url_with_options(
        url: &str,
        key_prefix: &str,
        pool_size: u32,
        pool_timeout_secs: u64,
        idle_timeout_secs: u64,
        min_idle: u32,
    ) -> Result<Self, StorageError> {
        let pool = RedisPool::standalone(
            url,
            pool_size,
            pool_timeout_secs,
            idle_timeout_secs,
            min_idle,
        )?;
        Ok(Self {
            pool,
            replica_pool: None,
            key_prefix: key_prefix.to_string(),
            read_consistency: ReadConsistency::TopologyDefault,
        })
    }

    /// Create a store connecting to a Redis Cluster via seed node URLs.
    ///
    /// The cluster client auto-discovers the full topology from the seed nodes.
    /// All keys for a given graph route to the same hash slot via `{graphname}`
    /// hash tags in the key schema.
    pub fn from_cluster_nodes(
        nodes: &[&str],
        key_prefix: &str,
        pool_size: u32,
        pool_timeout_secs: u64,
        idle_timeout_secs: u64,
        min_idle: u32,
    ) -> Result<Self, StorageError> {
        let pool = RedisPool::cluster(
            nodes,
            pool_size,
            pool_timeout_secs,
            idle_timeout_secs,
            min_idle,
        )?;
        Ok(Self {
            pool,
            replica_pool: None,
            key_prefix: key_prefix.to_string(),
            read_consistency: ReadConsistency::TopologyDefault,
        })
    }

    /// Create a store connecting to an explicit Redis URL with no key prefix.
    pub fn from_url(url: &str) -> Result<Self, StorageError> {
        Self::from_url_with_prefix(url, "")
    }

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

    /// Check graph existence against the provided connection.
    pub(crate) fn graph_exists_on(
        &self,
        conn: &mut dyn ConnectionLike,
        g: &str,
    ) -> Result<bool, StorageError> {
        let registered: bool = redis::cmd("SISMEMBER")
            .arg(self.key_graphs())
            .arg(g)
            .query(conn)
            .map_err(backend_storage_error)?;
        if !registered {
            return Ok(false);
        }
        Ok(!self.graph_is_deleting_on(conn, g)?)
    }

    /// Return `true` while `delete_graph` cleanup is in progress for `g`.
    pub(crate) fn graph_is_deleting_on(
        &self,
        conn: &mut dyn ConnectionLike,
        g: &str,
    ) -> Result<bool, StorageError> {
        let deleting_key = self.kp(key_graph_deleting(g));
        let exists: i64 = redis::cmd("EXISTS")
            .arg(&deleting_key)
            .query(conn)
            .map_err(backend_storage_error)?;
        Ok(exists > 0)
    }

    /// Check that a graph exists on the provided connection; return error if not.
    pub(crate) fn require_graph_on(
        &self,
        conn: &mut dyn ConnectionLike,
        g: &str,
    ) -> Result<(), StorageError> {
        if !self.graph_exists_on(conn, g)? {
            return Err(StorageError::GraphNotFound {
                name: g.to_string(),
            });
        }
        Ok(())
    }

    /// Apply the configured key prefix to `key`.
    ///
    /// When `key_prefix` is empty (the default) this is a no-op clone.
    #[inline]
    pub(crate) fn kp(&self, key: String) -> String {
        if self.key_prefix.is_empty() {
            key
        } else {
            format!("{}{}", self.key_prefix, key)
        }
    }

    /// Return the prefixed graph-registry key.
    #[inline]
    pub(crate) fn key_graphs(&self) -> String {
        self.kp(KEY_GRAPHS.to_string())
    }

    fn try_acquire_write_locks(
        &self,
        conn: &mut RedisConn,
        graph: &str,
        lock_keys: &[String],
        ttl_ms: u64,
    ) -> Result<Option<Vec<(String, String)>>, StorageError> {
        let mut held = Vec::with_capacity(lock_keys.len());
        for lock_key in lock_keys {
            match lock::acquire(conn.as_mut(), lock_key, ttl_ms) {
                Ok(Some(token)) => held.push((lock_key.clone(), token)),
                Ok(None) => {
                    let mut cleanup = Ok(());
                    while let Some((held_key, held_token)) = held.pop() {
                        cleanup = map_lock_release_result(
                            graph,
                            lock::release(conn.as_mut(), &held_key, &held_token),
                            cleanup,
                        );
                    }
                    cleanup?;
                    return Ok(None);
                }
                Err(err) => {
                    let mut cleanup: Result<(), StorageError> = Err(err);
                    while let Some((held_key, held_token)) = held.pop() {
                        cleanup = map_lock_release_result(
                            graph,
                            lock::release(conn.as_mut(), &held_key, &held_token),
                            cleanup,
                        );
                    }
                    return cleanup
                        .map(|_| unreachable!("cleanup after write-lock acquire failure"));
                }
            }
        }
        Ok(Some(held))
    }

    /// Acquire the graph write lock plus the requested scoped lock (if any),
    /// run `f`, then release them in reverse order.
    ///
    /// This still serializes same-graph writes through the graph lock. The
    /// scoped key is an additional ownership marker for hot mutators, not a
    /// same-graph parallelism primitive.
    pub(crate) fn with_write_lock<T, F>(
        &self,
        graph: &str,
        lock_key: String,
        ttl_ms: u64,
        max_retries: u32,
        retry_delay_ms: u64,
        f: F,
    ) -> Result<T, StorageError>
    where
        F: FnOnce(&mut LockedRedisConn) -> Result<T, StorageError>,
    {
        let mut work = Some(f);
        let lock_started_at = Instant::now();
        let mut saw_contention = false;
        let graph_lock_key = self.kp(key_graph_lock(graph));
        let lock_keys = if lock_key == graph_lock_key {
            vec![lock_key]
        } else {
            vec![graph_lock_key, lock_key]
        };
        metrics::counter!(REDIS_WRITE_LOCK_ACQUIRE_ATTEMPTS_TOTAL).increment(1);
        for attempt in 0..=max_retries {
            let mut conn = match self.conn() {
                Ok(conn) => conn,
                Err(err) => {
                    if saw_contention {
                        metrics::counter!(REDIS_WRITE_LOCK_ACQUIRE_CONTENTION_TOTAL).increment(1);
                    }
                    metrics::histogram!(REDIS_WRITE_LOCK_ACQUIRE_WAIT_SECONDS)
                        .record(lock_started_at.elapsed().as_secs_f64());
                    record_write_lock_backend_failure("acquire");
                    return Err(err);
                }
            };
            match self.try_acquire_write_locks(&mut conn, graph, &lock_keys, ttl_ms) {
                Ok(Some(locks)) => {
                    if saw_contention {
                        metrics::counter!(REDIS_WRITE_LOCK_ACQUIRE_CONTENTION_TOTAL).increment(1);
                    }
                    metrics::counter!(REDIS_WRITE_LOCK_ACQUIRE_SUCCESSES_TOTAL).increment(1);
                    metrics::histogram!(REDIS_WRITE_LOCK_ACQUIRE_WAIT_SECONDS)
                        .record(lock_started_at.elapsed().as_secs_f64());
                    let mut locked_conn = LockedRedisConn::new(conn, graph, locks, ttl_ms);
                    let result = work
                        .take()
                        .expect("write-lock closure must execute at most once")(
                        &mut locked_conn
                    );
                    return locked_conn.finish(result);
                }
                Ok(None) => {
                    saw_contention = true;
                }
                Err(err) => {
                    if saw_contention {
                        metrics::counter!(REDIS_WRITE_LOCK_ACQUIRE_CONTENTION_TOTAL).increment(1);
                    }
                    metrics::histogram!(REDIS_WRITE_LOCK_ACQUIRE_WAIT_SECONDS)
                        .record(lock_started_at.elapsed().as_secs_f64());
                    record_write_lock_backend_failure("acquire");
                    return Err(err);
                }
            }
            if attempt < max_retries {
                drop(conn);
                std::thread::sleep(std::time::Duration::from_millis(retry_delay_ms));
                continue;
            }
            metrics::counter!(REDIS_WRITE_LOCK_ACQUIRE_CONTENTION_TOTAL).increment(1);
            metrics::counter!(REDIS_WRITE_LOCK_CONTENTION_FAILURES_TOTAL).increment(1);
            metrics::histogram!(REDIS_WRITE_LOCK_ACQUIRE_WAIT_SECONDS)
                .record(lock_started_at.elapsed().as_secs_f64());
            return Err(StorageError::Backend {
                msg: format!(
                    "write locks '{}' still contended after {max_retries} retries",
                    lock_keys.join(", ")
                ),
            });
        }
        unreachable!("write-lock retry loop must return or error");
    }

    /// Acquire the per-graph write lock, run `f`, then release the lock.
    pub(crate) fn with_graph_write_lock<T, F>(
        &self,
        graph: &str,
        ttl_ms: u64,
        max_retries: u32,
        retry_delay_ms: u64,
        f: F,
    ) -> Result<T, StorageError>
    where
        F: FnOnce(&mut LockedRedisConn) -> Result<T, StorageError>,
    {
        self.with_write_lock(
            graph,
            self.kp(key_graph_lock(graph)),
            ttl_ms,
            max_retries,
            retry_delay_ms,
            f,
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn connection_defaults_are_sensible() {
        use crate::connection::*;
        const { assert!(DEFAULT_POOL_SIZE > 0) };
        const { assert!(DEFAULT_POOL_TIMEOUT_SECS > 0) };
        const { assert!(DEFAULT_IDLE_TIMEOUT_SECS > 0) };
        const { assert!(DEFAULT_MIN_IDLE > 0) };
        // Pool size should be reasonable (not absurdly large).
        const { assert!(DEFAULT_POOL_SIZE <= 256) };
    }
}
