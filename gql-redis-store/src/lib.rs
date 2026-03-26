// gql-redis-store: Redis/Valkey-backed implementation of the GraphStore trait.
//
// Key schema (all keys use {graphname} hash tags for Redis Cluster slot affinity):
//   Node hash:       G:{g}:node:{id}        — HSET labels (JSON), props (JSON)
//   Edge hash:       G:{g}:edge:{id}        — HSET src, dst, rel_type, props (JSON)
//   Node counter:    G:{g}:node_seq         — INCR
//   Edge counter:    G:{g}:edge_seq         — INCR
//   Label index:     G:{g}:label:{label}    — SADD node id (string)
//   RelType index:   G:{g}:reltype:{type}   — SADD edge id (string)
//   Out-adjacency:   G:{g}:out:{node_id}    — SADD "{edge_id}:{neighbor_id}:{rel_type}"
//   In-adjacency:    G:{g}:in:{node_id}     — SADD "{edge_id}:{neighbor_id}:{rel_type}"
//   All nodes set:   G:{g}:nodes            — SADD node id (string)
//   All edges set:   G:{g}:edges            — SADD edge id (string)
//   Index set:       G:{g}:indexes          — SADD "{label}\x00{property}"
//   Delete marker:   G:{g}:_deleting        — SET while delete_graph cleanup is in progress
//   Propidx manifest:G:{g}:_propidx         — SADD fully-qualified propidx keys for delete_graph
//   Graph registry:  __sd2_registry:graphs  — SADD graph name

pub mod create_store;
pub mod fanout;
pub(crate) mod lock;
pub mod redis_memory_metrics;

mod audit;
mod connection;
mod indexing;
mod keys;
mod range;
mod reads;
mod validation;
mod writes;

pub use audit::GraphMetadataAuditReport;
pub use connection::PoolStats;
pub use fanout::FanoutStore;

/// Upper bound on nodes a graph may contain before `delete_graph` is refused.
///
/// Deletion enumerates the all-nodes and all-edges sets via SMEMBERS, then
/// issues explicit DEL commands — safe on Redis Cluster because every key
/// for the same graph hashes to the same slot.  Graphs larger than this
/// limit must be deleted in parts or via an offline tool.
pub const MAX_NODES_FOR_DELETE: usize = 100_000;

use indexmap::IndexMap;
use std::env;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

// ── Backend-error counter ────────────────────────────────────────────────────

/// Global count of `StorageError::Backend` occurrences.
static BACKEND_ERROR_COUNT: AtomicU64 = AtomicU64::new(0);

const REDIS_WRITE_LOCK_ACQUIRE_ATTEMPTS_TOTAL: &str = "redis_write_lock_acquire_attempts_total";
const REDIS_WRITE_LOCK_ACQUIRE_SUCCESSES_TOTAL: &str = "redis_write_lock_acquire_successes_total";
const REDIS_WRITE_LOCK_ACQUIRE_CONTENTION_TOTAL: &str = "redis_write_lock_acquire_contention_total";
const REDIS_WRITE_LOCK_CONTENTION_FAILURES_TOTAL: &str =
    "redis_write_lock_contention_failures_total";
const REDIS_WRITE_LOCK_ACQUIRE_WAIT_SECONDS: &str = "redis_write_lock_acquire_wait_seconds";
const REDIS_WRITE_LOCK_RENEW_SUCCESSES_TOTAL: &str = "redis_write_lock_renew_successes_total";
const REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL: &str = "redis_write_lock_renew_failures_total";
const REDIS_WRITE_LOCK_RELEASE_SUCCESSES_TOTAL: &str = "redis_write_lock_release_successes_total";
const REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL: &str = "redis_write_lock_release_failures_total";
const REDIS_WRITE_LOCK_HOLD_SECONDS: &str = "redis_write_lock_hold_seconds";
const REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL: &str = "redis_write_lock_conflict_outcomes_total";
const REDIS_WRITE_LOCK_BACKEND_FAILURES_TOTAL: &str = "redis_write_lock_backend_failures_total";

/// Increment the `StorageError::Backend` counter by one.
pub fn record_backend_error() {
    BACKEND_ERROR_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Return the current `StorageError::Backend` count.
pub fn backend_error_count() -> u64 {
    BACKEND_ERROR_COUNT.load(Ordering::Relaxed)
}

fn record_write_lock_conflict_outcome(stage: &'static str) {
    metrics::counter!(REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL, "stage" => stage).increment(1);
}

fn record_write_lock_backend_failure(stage: &'static str) {
    metrics::counter!(REDIS_WRITE_LOCK_BACKEND_FAILURES_TOTAL, "stage" => stage).increment(1);
}

/// Sanitize a backend error message for inclusion in `StorageError::Backend`.
///
/// In debug builds the full message is returned so engineers can see the
/// underlying Redis error detail.  In release builds the message is replaced
/// with a fixed string so internal Redis topology information (e.g. MOVED
/// redirects with cluster node IPs) is not leaked to callers (SEC-6).
#[cfg(debug_assertions)]
pub fn backend_error_msg(msg: &str) -> String {
    msg.to_string()
}

/// Sanitize a backend error message for inclusion in `StorageError::Backend`.
///
/// In release builds the message is replaced with a fixed string so internal
/// Redis topology information is not leaked to callers (SEC-6).
#[cfg(not(debug_assertions))]
pub fn backend_error_msg(_msg: &str) -> String {
    "backend error".to_string()
}

pub(crate) fn backend_storage_error<E: std::fmt::Display>(e: E) -> StorageError {
    record_backend_error();
    let msg = e.to_string();
    StorageError::Backend {
        msg: backend_error_msg(&msg),
    }
}

use redis::ConnectionLike;

use gql_core::error::StorageError;
use gql_core::serde_value::json_to_props;
use gql_core::storage::{
    AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor, NodeData, NodeId,
    RangeQuery, ReadConsistency,
};
use gql_core::types::Value;

use connection::{
    RedisConn, RedisPool, ReplicaPool, DEFAULT_IDLE_TIMEOUT_SECS, DEFAULT_MIN_IDLE,
    DEFAULT_POOL_SIZE, DEFAULT_POOL_TIMEOUT_SECS,
};
use keys::{decode_adj, key_edge, key_graph_deleting, key_graph_lock, key_node, KEY_GRAPHS};

struct HeldRedisLock {
    key: Box<str>,
    token: Box<str>,
}

pub(crate) struct LockedRedisConn {
    conn: RedisConn,
    graph: Box<str>,
    locks: Vec<HeldRedisLock>,
    ttl_ms: u64,
    acquired_at: Instant,
}

fn map_lock_renewal_result(
    graph: &str,
    renewal: Result<bool, StorageError>,
) -> Result<(), StorageError> {
    match renewal {
        Ok(true) => {
            metrics::counter!(REDIS_WRITE_LOCK_RENEW_SUCCESSES_TOTAL).increment(1);
            Ok(())
        }
        Ok(false) => {
            metrics::counter!(REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL).increment(1);
            record_write_lock_conflict_outcome("renew");
            tracing::warn!(
                graph = %graph,
                "graph write lock expired or changed owner before renewal"
            );
            Err(StorageError::TransactionConflict)
        }
        Err(err) => {
            metrics::counter!(REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL).increment(1);
            record_write_lock_backend_failure("renew");
            tracing::warn!(
                graph = %graph,
                error = %err,
                "graph write lock renewal failed"
            );
            Err(err)
        }
    }
}

fn map_lock_release_result<T>(
    graph: &str,
    release: Result<bool, StorageError>,
    result: Result<T, StorageError>,
) -> Result<T, StorageError> {
    match release {
        Ok(true) => {
            metrics::counter!(REDIS_WRITE_LOCK_RELEASE_SUCCESSES_TOTAL).increment(1);
            result
        }
        Ok(false) => {
            metrics::counter!(REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL).increment(1);
            tracing::warn!(
                graph = %graph,
                "graph write lock expired or changed owner before release"
            );
            let result = match result {
                Ok(_) => Err(StorageError::TransactionConflict),
                Err(err) => Err(err),
            };
            if matches!(&result, Err(StorageError::TransactionConflict)) {
                record_write_lock_conflict_outcome("release");
            }
            result
        }
        Err(err) => {
            metrics::counter!(REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL).increment(1);
            record_write_lock_backend_failure("release");
            tracing::warn!(
                graph = %graph,
                error = %err,
                "graph write lock release failed"
            );
            match result {
                Ok(_) => Err(err),
                Err(work_err) => Err(work_err),
            }
        }
    }
}

impl LockedRedisConn {
    fn new(conn: RedisConn, graph: &str, locks: Vec<(String, String)>, ttl_ms: u64) -> Self {
        Self {
            conn,
            graph: graph.into(),
            locks: locks
                .into_iter()
                .map(|(key, token)| HeldRedisLock {
                    key: key.into_boxed_str(),
                    token: token.into_boxed_str(),
                })
                .collect(),
            ttl_ms,
            acquired_at: Instant::now(),
        }
    }

    pub(crate) fn renew_lock(&mut self) -> Result<(), StorageError> {
        for held_lock in &self.locks {
            map_lock_renewal_result(
                self.graph.as_ref(),
                lock::renew(
                    self.conn.as_mut(),
                    held_lock.key.as_ref(),
                    held_lock.token.as_ref(),
                    self.ttl_ms,
                ),
            )?;
        }
        Ok(())
    }

    fn finish<T>(mut self, result: Result<T, StorageError>) -> Result<T, StorageError> {
        metrics::histogram!(REDIS_WRITE_LOCK_HOLD_SECONDS)
            .record(self.acquired_at.elapsed().as_secs_f64());
        let mut result = result;
        for held_lock in self.locks.iter().rev() {
            result = map_lock_release_result(
                self.graph.as_ref(),
                lock::release(
                    self.conn.as_mut(),
                    held_lock.key.as_ref(),
                    held_lock.token.as_ref(),
                ),
                result,
            );
        }
        result
    }
}

impl Deref for LockedRedisConn {
    type Target = RedisConn;

    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

impl DerefMut for LockedRedisConn {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.conn
    }
}

// ── RedisGraphStore ─────────────────────────────────────────────────────────

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
/// - `GQL_REDIS_URL` / `REDIS_URL` — standalone URL (fallback `redis://127.0.0.1:6379`)
///
/// Pool configuration:
/// - `GQL_REDIS_POOL_SIZE` — max connections (default 64)
/// - `GQL_REDIS_POOL_TIMEOUT_SECS` — checkout timeout (default 5)
/// - `GQL_REDIS_IDLE_TIMEOUT_SECS` — idle connection reap timeout (default 300)
/// - `GQL_REDIS_MIN_IDLE` — minimum warm connections kept in pool (default 1)
///
/// Read-replica configuration:
/// - `GQL_REDIS_REPLICA_URL` — comma-separated replica URLs for read routing
#[derive(Clone)]
pub struct RedisGraphStore {
    pool: RedisPool,
    /// Optional read-replica pool. When present, `route_read()` returns
    /// connections from replicas; when absent, falls back to the primary pool.
    replica_pool: Option<ReplicaPool>,
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
    /// seed-node URLs).  Otherwise falls back to standalone mode using `GQL_REDIS_URL`,
    /// `REDIS_URL`, or `redis://127.0.0.1:6379`.
    ///
    /// The key prefix is read from `GQL_REDIS_KEY_PREFIX` (default empty string).
    /// Set a prefix to isolate multiple deployments sharing a single Redis instance.
    ///
    /// Pool size and timeout are read from `GQL_REDIS_POOL_SIZE` (default 64)
    /// and `GQL_REDIS_POOL_TIMEOUT_SECS` (default 5).
    /// Idle timeout and min-idle are read from `GQL_REDIS_IDLE_TIMEOUT_SECS` (default 300)
    /// and `GQL_REDIS_MIN_IDLE` (default 1).
    pub fn from_env() -> Result<Self, StorageError> {
        let prefix = env::var("GQL_REDIS_KEY_PREFIX").unwrap_or_default();
        let pool_size: u32 =
            gql_core::config::parse_env_range("GQL_REDIS_POOL_SIZE", DEFAULT_POOL_SIZE, 1, 1024);
        let pool_timeout_secs: u64 = gql_core::config::parse_env_range(
            "GQL_REDIS_POOL_TIMEOUT_SECS",
            DEFAULT_POOL_TIMEOUT_SECS,
            1,
            300,
        );
        let idle_timeout_secs: u64 = gql_core::config::parse_env_range(
            "GQL_REDIS_IDLE_TIMEOUT_SECS",
            DEFAULT_IDLE_TIMEOUT_SECS,
            0,
            86400,
        );
        let min_idle: u32 =
            gql_core::config::parse_env_range("GQL_REDIS_MIN_IDLE", DEFAULT_MIN_IDLE, 0, pool_size);

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

        // Standalone mode.
        let url = env::var("GQL_REDIS_URL")
            .or_else(|_| env::var("REDIS_URL"))
            .unwrap_or_else(|_| {
                tracing::warn!("GQL_REDIS_URL not set, falling back to redis://127.0.0.1:6379");
                "redis://127.0.0.1:6379".into()
            });
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
            .map_err(crate::backend_storage_error)?;
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
            .map_err(crate::backend_storage_error)?;
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

    /// Read a node hash from Redis; returns `None` if the key is absent.
    pub(crate) fn read_node(
        &self,
        conn: &mut dyn ConnectionLike,
        g: &str,
        id: u64,
    ) -> Result<Option<NodeData>, StorageError> {
        let key = self.kp(key_node(g, id));
        let fields: Vec<Option<String>> = redis::cmd("HMGET")
            .arg(&key)
            .arg("labels")
            .arg("props")
            .query(conn)
            .map_err(crate::backend_storage_error)?;
        match (&fields[0], &fields[1]) {
            (None, None) => Ok(None),
            (Some(labels_json), Some(props_json)) => {
                let labels: Vec<String> =
                    serde_json::from_str(labels_json).map_err(|e| StorageError::Serialization {
                        msg: format!("labels: {e}"),
                    })?;
                let properties = json_to_props(props_json)?;
                Ok(Some(NodeData {
                    id: NodeId(id),
                    labels,
                    properties,
                }))
            }
            _ => {
                tracing::error!(
                    node_id = id,
                    graph = g,
                    labels = ?fields[0],
                    props = ?fields[1],
                    "partial node data detected — possible data corruption"
                );
                record_backend_error();
                Err(StorageError::Backend {
                    msg: format!(
                        "corrupted node data: node {id} in graph '{g}' has partial fields"
                    ),
                })
            }
        }
    }

    /// Read an edge hash from Redis; returns `None` if the key is absent.
    pub(crate) fn read_edge(
        &self,
        conn: &mut dyn ConnectionLike,
        g: &str,
        id: u64,
    ) -> Result<Option<EdgeData>, StorageError> {
        let key = self.kp(key_edge(g, id));
        let fields: Vec<Option<String>> = redis::cmd("HMGET")
            .arg(&key)
            .arg("src")
            .arg("dst")
            .arg("rel_type")
            .arg("props")
            .query(conn)
            .map_err(crate::backend_storage_error)?;
        match (&fields[0], &fields[1], &fields[2], &fields[3]) {
            (Some(src_s), Some(dst_s), Some(rel_type), Some(props_json)) => {
                let src =
                    NodeId(
                        src_s
                            .parse::<u64>()
                            .map_err(|_| StorageError::Serialization {
                                msg: "bad src id".into(),
                            })?,
                    );
                let dst =
                    NodeId(
                        dst_s
                            .parse::<u64>()
                            .map_err(|_| StorageError::Serialization {
                                msg: "bad dst id".into(),
                            })?,
                    );
                let properties = json_to_props(props_json)?;
                Ok(Some(EdgeData {
                    id: EdgeId(id),
                    rel_type: rel_type.clone(),
                    src,
                    dst,
                    properties,
                }))
            }
            (None, None, None, None) => Ok(None),
            _ => {
                tracing::error!(
                    edge_id = id,
                    graph = g,
                    src = ?fields[0],
                    dst = ?fields[1],
                    rel_type = ?fields[2],
                    props = ?fields[3],
                    "partial edge data detected — possible data corruption"
                );
                record_backend_error();
                Err(StorageError::Backend {
                    msg: format!(
                        "corrupted edge data: edge {id} in graph '{g}' has partial fields"
                    ),
                })
            }
        }
    }

    /// Parse adjacency set members into `AdjEntry` list.
    pub(crate) fn parse_adj_members(members: Vec<String>) -> Vec<AdjEntry> {
        let mut entries = Vec::new();
        for m in members {
            match decode_adj(&m) {
                Some(entry) => entries.push(entry),
                None => {
                    tracing::warn!(member = %m, "failed to decode adjacency member, skipping");
                }
            }
        }
        entries
    }
}

// ── GraphStore impl ─────────────────────────────────────────────────────────

impl GraphStore for RedisGraphStore {
    // ── Single-item reads ────────────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        self.get_node_impl(graph, id)
    }

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        self.get_edge_impl(graph, id)
    }

    // ── Batch reads ──────────────────────────────────────────────────────────

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        self.get_nodes_batch_impl(graph, ids)
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        self.get_edges_batch_impl(graph, ids)
    }

    // ── Label/type index reads ───────────────────────────────────────────────

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        self.find_nodes_by_label_impl(graph, label)
    }

    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        self.find_nodes_by_label_and_property_impl(graph, label, property, range)
    }

    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.find_edges_by_type_impl(graph, rel_type)
    }

    // ── Adjacency reads ─────────────────────────────────────────────────────

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.out_edges_impl(graph, node_id)
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.in_edges_impl(graph, node_id)
    }

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.out_edges_by_type_impl(graph, node_id, rel_types)
    }

    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.in_edges_by_type_impl(graph, node_id, rel_types)
    }

    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.out_edge_ids_impl(graph, node_id)
    }

    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.in_edge_ids_impl(graph, node_id)
    }

    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        self.all_node_ids_impl(graph)
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.all_edge_ids_impl(graph)
    }

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        self.all_label_names_impl(graph)
    }

    // ── Write operations ────────────────────────────────────────────────────

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        self.create_node_impl(graph, labels, properties)
    }

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        self.create_edge_impl(graph, src, dst, rel_type, properties)
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.set_node_properties_impl(graph, id, properties)
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.replace_node_properties_impl(graph, id, properties)
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.set_edge_properties_impl(graph, id, properties)
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.replace_edge_properties_impl(graph, id, properties)
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        self.remove_node_property_impl(graph, id, key)
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        self.remove_edge_property_impl(graph, id, key)
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.add_node_labels_impl(graph, id, labels)
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.remove_node_labels_impl(graph, id, labels)
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        self.delete_node_impl(graph, id, detach)
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.delete_edge_impl(graph, id)
    }

    // ── Index management ────────────────────────────────────────────────────

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        self.list_indexes_impl(graph)
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.create_index_impl(graph, label, property)
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.drop_index_impl(graph, label, property)
    }

    // ── Graph administration ────────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        self.create_graph_impl(name)
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        self.delete_graph_impl(name)
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        self.graph_exists_impl(name)
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        self.list_graphs_impl()
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        self.graph_stats_impl(graph)
    }

    fn with_read_consistency(&self, consistency: ReadConsistency) -> Option<Arc<dyn GraphStore>> {
        Some(Arc::new(
            self.clone().with_read_consistency_mode(consistency),
        ))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    // tx_snapshot and tx_commit use the default no-op implementations.
    // RedisGraphStore writes are immediately durable — no client-side
    // snapshot is needed. Redis Cluster mode can add WATCH/MULTI/EXEC
    // for per-slot transaction isolation in a future iteration.
}

#[cfg(test)]
mod tests {
    use super::*;
    use keys::encode_adj;

    // ── parse_adj_members ───────────────────────────────────────────────────

    #[test]
    fn parse_adj_members_valid_entries() {
        let members = vec!["1:2:KNOWS".to_string(), "3:4:LIKES".to_string()];
        let entries = RedisGraphStore::parse_adj_members(members);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].edge_id, EdgeId(1));
        assert_eq!(entries[0].neighbor_id, NodeId(2));
        assert_eq!(entries[0].rel_type, "KNOWS");
        assert_eq!(entries[1].edge_id, EdgeId(3));
        assert_eq!(entries[1].neighbor_id, NodeId(4));
        assert_eq!(entries[1].rel_type, "LIKES");
    }

    #[test]
    fn parse_adj_members_skips_malformed() {
        let members = vec![
            "1:2:KNOWS".to_string(),
            "bad_entry".to_string(), // malformed — should be filtered out
            "3:4:LIKES".to_string(),
        ];
        let entries = RedisGraphStore::parse_adj_members(members);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn parse_adj_members_empty_input() {
        let entries = RedisGraphStore::parse_adj_members(vec![]);
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_adj_members_all_malformed() {
        let members = vec!["bad".to_string(), "also_bad".to_string()];
        let entries = RedisGraphStore::parse_adj_members(members);
        assert!(entries.is_empty());
    }

    // ── encode_adj / decode_adj integration with parse_adj_members ──────────

    #[test]
    fn encode_then_parse_round_trip() {
        let encoded = vec![
            encode_adj(EdgeId(10), NodeId(20), "FOLLOWS"),
            encode_adj(EdgeId(11), NodeId(21), "BLOCKS"),
        ];
        let entries = RedisGraphStore::parse_adj_members(encoded);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].edge_id, EdgeId(10));
        assert_eq!(entries[0].neighbor_id, NodeId(20));
        assert_eq!(entries[0].rel_type, "FOLLOWS");
        assert_eq!(entries[1].edge_id, EdgeId(11));
        assert_eq!(entries[1].neighbor_id, NodeId(21));
        assert_eq!(entries[1].rel_type, "BLOCKS");
    }

    // ── key_graphs helper ───────────────────────────────────────────────────
    // We cannot construct RedisGraphStore without a live Redis pool, but we
    // can test the key_prefix logic via the `kp` function by checking its
    // contract: if prefix is empty, return key unchanged; otherwise prepend.
    // These tests exercise the `kp` logic indirectly via `key_graphs` by
    // verifying the expected key format using the constant.

    #[test]
    fn key_graphs_constant_value() {
        assert_eq!(keys::KEY_GRAPHS, "__sd2_registry:graphs");
    }

    // ── Key format smoke tests ──────────────────────────────────────────────
    // Verify that the key schema documented in the module comment is accurate.

    #[test]
    fn key_schema_node_hash() {
        assert_eq!(keys::key_node("mydb", 1), "G:{mydb}:node:1");
    }

    #[test]
    fn key_schema_edge_hash() {
        assert_eq!(keys::key_edge("mydb", 5), "G:{mydb}:edge:5");
    }

    #[test]
    fn key_schema_label_index() {
        assert_eq!(keys::key_label("mydb", "Person"), "G:{mydb}:label:Person");
    }

    #[test]
    fn key_schema_reltype_index() {
        assert_eq!(keys::key_reltype("mydb", "KNOWS"), "G:{mydb}:reltype:KNOWS");
    }

    #[test]
    fn key_schema_out_adjacency() {
        assert_eq!(keys::key_out("mydb", 7), "G:{mydb}:out:7");
    }

    #[test]
    fn key_schema_in_adjacency() {
        assert_eq!(keys::key_in("mydb", 7), "G:{mydb}:in:7");
    }

    // ── Connection defaults ─────────────────────────────────────────────────

    #[test]
    fn connection_defaults_are_sensible() {
        use connection::*;
        const { assert!(DEFAULT_POOL_SIZE > 0) };
        const { assert!(DEFAULT_POOL_TIMEOUT_SECS > 0) };
        const { assert!(DEFAULT_IDLE_TIMEOUT_SECS > 0) };
        const { assert!(DEFAULT_MIN_IDLE > 0) };
        // Pool size should be reasonable (not absurdly large).
        const { assert!(DEFAULT_POOL_SIZE <= 256) };
    }
}

// ── Backend error counter tests ──────────────────────────────────────────────

#[cfg(test)]
mod backend_error_counter_tests {
    use super::*;

    #[test]
    fn backend_error_count_starts_at_zero_or_valid() {
        // Note: counter may be > 0 if other tests ran first.
        // Just verify it's a valid u64.
        let _ = backend_error_count();
    }

    #[test]
    fn record_backend_error_increments_counter() {
        let before = backend_error_count();
        record_backend_error();
        let after = backend_error_count();
        assert_eq!(after, before + 1, "counter should increment by 1");
    }
}

#[cfg(test)]
mod write_lock_outcome_tests {
    use super::*;
    use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
    use std::sync::{Mutex, OnceLock};

    fn metrics_handle() -> &'static PrometheusHandle {
        static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
        HANDLE.get_or_init(|| {
            PrometheusBuilder::new()
                .install_recorder()
                .expect("failed to install test Prometheus recorder")
        })
    }

    fn metrics_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock()
            .expect("metrics test lock should not be poisoned")
    }

    fn metric_value(handle: &PrometheusHandle, metric: &str, labels: &[(&str, &str)]) -> f64 {
        let output = handle.render();
        output
            .lines()
            .find_map(|line| {
                if line.starts_with('#') {
                    return None;
                }
                let rest = line.strip_prefix(metric)?;
                if !matches!(rest.chars().next(), Some(' ') | Some('{')) {
                    return None;
                }
                if labels.is_empty() && rest.starts_with('{') {
                    return None;
                }
                if !labels
                    .iter()
                    .all(|(key, value)| label_matches(line, key, value))
                {
                    return None;
                }
                line.split_whitespace().last()?.parse::<f64>().ok()
            })
            .unwrap_or(0.0)
    }

    fn label_matches(line: &str, key: &str, value: &str) -> bool {
        let expected = format!(r#"{key}="{value}""#);
        line.find(&expected).is_some_and(|index| {
            let bytes = line.as_bytes();
            let leading_ok = index == 0 || matches!(bytes.get(index - 1), Some(b'{') | Some(b','));
            let trailing_ok = matches!(bytes.get(index + expected.len()), Some(b',') | Some(b'}'));
            leading_ok && trailing_ok
        })
    }

    #[test]
    fn lost_lock_renewal_maps_to_transaction_conflict() {
        let _guard = metrics_guard();
        let result = map_lock_renewal_result("lease-loss", Ok(false));
        assert!(matches!(result, Err(StorageError::TransactionConflict)));
    }

    #[test]
    fn lost_lock_release_turns_successful_write_into_transaction_conflict() {
        let _guard = metrics_guard();
        let result = map_lock_release_result("lease-loss", Ok(false), Ok(()));
        assert!(matches!(result, Err(StorageError::TransactionConflict)));
    }

    #[test]
    fn lost_lock_release_preserves_underlying_write_error() {
        let _guard = metrics_guard();
        let result: Result<(), StorageError> = map_lock_release_result(
            "lease-loss",
            Ok(false),
            Err(StorageError::GraphNotFound {
                name: "missing".into(),
            }),
        );
        assert!(matches!(result, Err(StorageError::GraphNotFound { .. })));
    }

    #[test]
    fn release_error_after_successful_write_surfaces_release_error() {
        let _guard = metrics_guard();
        let result: Result<(), StorageError> = map_lock_release_result(
            "lease-loss",
            Err(StorageError::Backend {
                msg: "redis release failed".into(),
            }),
            Ok(()),
        );
        assert!(matches!(
            result,
            Err(StorageError::Backend { msg }) if msg == "redis release failed"
        ));
    }

    #[test]
    fn release_error_preserves_underlying_write_error() {
        let _guard = metrics_guard();
        let result: Result<(), StorageError> = map_lock_release_result(
            "lease-loss",
            Err(StorageError::Backend {
                msg: "redis release failed".into(),
            }),
            Err(StorageError::GraphNotFound {
                name: "missing".into(),
            }),
        );
        assert!(matches!(result, Err(StorageError::GraphNotFound { .. })));
    }

    #[test]
    fn renew_success_records_metric() {
        let _guard = metrics_guard();
        let handle = metrics_handle();
        let before = metric_value(handle, REDIS_WRITE_LOCK_RENEW_SUCCESSES_TOTAL, &[]);

        let result = map_lock_renewal_result("lease-loss", Ok(true));

        assert!(result.is_ok());
        let after = metric_value(handle, REDIS_WRITE_LOCK_RENEW_SUCCESSES_TOTAL, &[]);
        assert!(
            after >= before + 1.0,
            "renew success counter should increase (before={before}, after={after})"
        );
    }

    #[test]
    fn renew_conflict_records_failure_and_conflict_metrics() {
        let _guard = metrics_guard();
        let handle = metrics_handle();
        let before_failures = metric_value(handle, REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL, &[]);
        let before_conflicts = metric_value(
            handle,
            REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
            &[("stage", "renew")],
        );

        let result = map_lock_renewal_result("lease-loss", Ok(false));

        assert!(matches!(result, Err(StorageError::TransactionConflict)));
        let after_failures = metric_value(handle, REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL, &[]);
        let after_conflicts = metric_value(
            handle,
            REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
            &[("stage", "renew")],
        );
        assert!(after_failures >= before_failures + 1.0);
        assert!(after_conflicts >= before_conflicts + 1.0);
    }

    #[test]
    fn renew_backend_error_records_failure_and_backend_metric() {
        let _guard = metrics_guard();
        let handle = metrics_handle();
        let before_failures = metric_value(handle, REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL, &[]);
        let before_backend = metric_value(
            handle,
            REDIS_WRITE_LOCK_BACKEND_FAILURES_TOTAL,
            &[("stage", "renew")],
        );
        let before_conflicts = metric_value(
            handle,
            REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
            &[("stage", "renew")],
        );

        let result = map_lock_renewal_result(
            "lease-loss",
            Err(StorageError::Backend {
                msg: "redis renew failed".into(),
            }),
        );

        assert!(matches!(result, Err(StorageError::Backend { .. })));
        let after_failures = metric_value(handle, REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL, &[]);
        let after_backend = metric_value(
            handle,
            REDIS_WRITE_LOCK_BACKEND_FAILURES_TOTAL,
            &[("stage", "renew")],
        );
        let after_conflicts = metric_value(
            handle,
            REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
            &[("stage", "renew")],
        );
        assert!(after_failures >= before_failures + 1.0);
        assert!(after_backend >= before_backend + 1.0);
        assert_eq!(
            after_conflicts, before_conflicts,
            "backend errors during renew must not emit conflict outcomes"
        );
    }

    #[test]
    fn release_success_records_metric() {
        let _guard = metrics_guard();
        let handle = metrics_handle();
        let before = metric_value(handle, REDIS_WRITE_LOCK_RELEASE_SUCCESSES_TOTAL, &[]);

        let result: Result<(), StorageError> =
            map_lock_release_result("lease-loss", Ok(true), Ok(()));

        assert!(result.is_ok());
        let after = metric_value(handle, REDIS_WRITE_LOCK_RELEASE_SUCCESSES_TOTAL, &[]);
        assert!(
            after >= before + 1.0,
            "release success counter should increase (before={before}, after={after})"
        );
    }

    #[test]
    fn release_conflict_records_failure_and_conflict_metrics() {
        let _guard = metrics_guard();
        let handle = metrics_handle();
        let before_failures = metric_value(handle, REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL, &[]);
        let before_conflicts = metric_value(
            handle,
            REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
            &[("stage", "release")],
        );

        let result: Result<(), StorageError> =
            map_lock_release_result("lease-loss", Ok(false), Ok(()));

        assert!(matches!(result, Err(StorageError::TransactionConflict)));
        let after_failures = metric_value(handle, REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL, &[]);
        let after_conflicts = metric_value(
            handle,
            REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
            &[("stage", "release")],
        );
        assert!(after_failures >= before_failures + 1.0);
        assert!(after_conflicts >= before_conflicts + 1.0);
    }

    #[test]
    fn release_backend_error_records_backend_failure_metric() {
        let _guard = metrics_guard();
        let handle = metrics_handle();
        let before_failures = metric_value(handle, REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL, &[]);
        let before_backend = metric_value(
            handle,
            REDIS_WRITE_LOCK_BACKEND_FAILURES_TOTAL,
            &[("stage", "release")],
        );

        let result: Result<(), StorageError> = map_lock_release_result(
            "lease-loss",
            Err(StorageError::Backend {
                msg: "redis release failed".into(),
            }),
            Ok(()),
        );

        assert!(matches!(result, Err(StorageError::Backend { .. })));
        let after_failures = metric_value(handle, REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL, &[]);
        let after_backend = metric_value(
            handle,
            REDIS_WRITE_LOCK_BACKEND_FAILURES_TOTAL,
            &[("stage", "release")],
        );
        assert!(after_failures >= before_failures + 1.0);
        assert!(after_backend >= before_backend + 1.0);
    }

    #[test]
    fn release_conflict_with_failed_write_does_not_record_conflict_outcome() {
        let _guard = metrics_guard();
        let handle = metrics_handle();
        let before_failures = metric_value(handle, REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL, &[]);
        let before_conflicts = metric_value(
            handle,
            REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
            &[("stage", "release")],
        );

        let result: Result<(), StorageError> = map_lock_release_result(
            "lease-loss",
            Ok(false),
            Err(StorageError::GraphNotFound {
                name: "missing".into(),
            }),
        );

        assert!(matches!(result, Err(StorageError::GraphNotFound { .. })));
        let after_failures = metric_value(handle, REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL, &[]);
        let after_conflicts = metric_value(
            handle,
            REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
            &[("stage", "release")],
        );
        assert!(after_failures >= before_failures + 1.0);
        assert_eq!(
            after_conflicts, before_conflicts,
            "release conflict outcomes should not increment when the write had already failed"
        );
    }
}
