use std::sync::Arc;

use cypher_core::storage::cached::CachedGraphStore;
use cypher_core::storage::{GraphStore, InMemoryGraphStore};

use crate::RedisGraphStore;

/// Optionally wrap a Redis-backed store with [`CachedGraphStore`].
///
/// Enabled when `GQL_CACHED_STORE=1` is set in the environment.
fn maybe_cached(store: RedisGraphStore) -> Arc<dyn GraphStore> {
    if std::env::var("GQL_CACHED_STORE").as_deref() == Ok("1") {
        tracing::info!("GQL_CACHED_STORE=1 — wrapping Redis store with in-memory cache");
        Arc::new(CachedGraphStore::new(Arc::new(store)))
    } else {
        Arc::new(store)
    }
}

/// Create the graph store based on environment configuration.
///
/// Priority: `GQL_REDIS_CLUSTER_NODES` (cluster) > `GQL_REDIS_URL` (standalone) > in-memory.
///
/// When a Redis URL is configured, connection failure is **fatal** — the caller
/// will receive a panic rather than silently degrading to in-memory mode.
///
/// When `GQL_CACHED_STORE=1` is also set, the Redis store is wrapped with
/// [`CachedGraphStore`] for in-memory read caching.
pub fn create_store() -> Arc<dyn GraphStore> {
    if std::env::var("GQL_REDIS_CLUSTER_NODES").is_ok() {
        tracing::info!("GQL_REDIS_CLUSTER_NODES set — using Redis Cluster-backed graph store");
        match RedisGraphStore::from_env() {
            Ok(store) => return maybe_cached(store),
            Err(e) => {
                panic!("GQL_REDIS_CLUSTER_NODES is set but connection failed: {e}");
            }
        }
    } else if std::env::var("GQL_REDIS_URL").is_ok() {
        tracing::info!("GQL_REDIS_URL set — using Redis-backed graph store");
        match RedisGraphStore::from_env() {
            Ok(store) => return maybe_cached(store),
            Err(e) => {
                panic!("GQL_REDIS_URL is set but connection failed: {e}");
            }
        }
    }
    tracing::warn!(
        "no Redis configured (GQL_REDIS_URL / GQL_REDIS_CLUSTER_NODES unset) \
         — using volatile in-memory store"
    );
    Arc::new(InMemoryGraphStore::new())
}
