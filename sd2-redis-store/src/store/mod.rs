// `RedisGraphStore` — the central store handle.
//
// Owns the primary and replica connection pools, the configured key prefix,
// and the session-scoped read-consistency contract. Exposes constructors,
// pool metrics, low-level connection helpers used by every other module in
// this crate, and the closure-based write-lock orchestration on top of which
// all mutating operations are layered.
//
// The implementation is split across sibling files for clarity:
//
// * [`config`]  — constructors (`from_env`, `from_url*`, `from_cluster_nodes`)
// * [`pool`]    — pool metrics and connection-routing helpers
// * [`registry`] — graph-registry lookups and key-prefix utilities
// * [`locks`]   — closure-based write-lock orchestration

use cypher_core::storage::ReadConsistency;

use crate::connection::{RedisPool, ReplicaPool};

mod config;
mod locks;
mod pool;
mod registry;

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
    pub(super) pool: RedisPool,
    /// Optional read-replica pool. When present, `route_read()` returns
    /// connections from replicas; when absent, falls back to the primary pool.
    pub(crate) replica_pool: Option<ReplicaPool>,
    /// Optional key prefix for multi-tenant isolation.
    /// Set via `GQL_REDIS_KEY_PREFIX` environment variable (default empty).
    pub(super) key_prefix: String,
    /// Session-scoped read-routing contract for this store view.
    pub(super) read_consistency: ReadConsistency,
}

#[cfg(test)]
impl RedisGraphStore {
    /// Construct a `RedisGraphStore` pointing at a bogus standalone URL for
    /// unit tests that exercise pure-Rust paths (key building, hash decoding,
    /// etc.) without opening a connection. `min_idle=0` prevents r2d2 from
    /// attempting an eager connect, so construction succeeds even when no
    /// Redis is reachable.
    pub(crate) fn new_for_testing() -> Self {
        let pool = crate::connection::RedisPool::standalone(
            "redis://127.0.0.1:1", // unreachable; never used
            1,
            1,
            60,
            0,
        )
        .expect("standalone pool construction should not require a live Redis");
        Self {
            pool,
            replica_pool: None,
            key_prefix: String::new(),
            read_consistency: cypher_core::storage::ReadConsistency::TopologyDefault,
        }
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
