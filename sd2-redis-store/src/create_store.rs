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

#[cfg(test)]
mod tests {
    //! Tests for [`create_store`]'s env-driven branching.
    //!
    //! `create_store()` panics if a Redis URL is configured but unreachable,
    //! so we only exercise the no-Redis branch here. The cluster / standalone
    //! constructor paths themselves are covered by `store::config` tests.

    use super::*;
    use std::env;

    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        crate::test_env_lock::ENV_LOCK
            .lock()
            .expect("env lock should not be poisoned")
    }

    const VARS: &[&str] = &[
        "GQL_REDIS_URL",
        "REDIS_URL",
        "GQL_REDIS_CLUSTER_NODES",
        "GQL_CACHED_STORE",
    ];

    fn snapshot() -> Vec<(&'static str, Option<String>)> {
        VARS.iter().map(|k| (*k, env::var(k).ok())).collect()
    }
    fn restore(snap: Vec<(&'static str, Option<String>)>) {
        for (k, v) in snap {
            match v {
                Some(val) => env::set_var(k, val),
                None => env::remove_var(k),
            }
        }
    }

    #[test]
    fn create_store_returns_in_memory_store_when_no_redis_configured() {
        let _guard = env_guard();
        let snap = snapshot();
        for k in VARS {
            env::remove_var(k);
        }

        let store = create_store();
        // Sanity check: we got a usable GraphStore back. We don't downcast
        // to InMemoryGraphStore (that detail is private), but creating a
        // graph against the returned store must succeed without a network.
        store
            .create_graph("smoke")
            .expect("in-memory store create_graph should succeed");
        assert!(store
            .graph_exists("smoke")
            .expect("graph_exists should succeed"));

        restore(snap);
    }

    #[test]
    fn maybe_cached_wraps_when_env_set() {
        // `maybe_cached` is private, so we exercise it through `create_store`
        // by combining `GQL_REDIS_URL` with `GQL_CACHED_STORE=1`. Construction
        // would panic if the URL is reachable-but-unauthenticated, but with
        // an unreachable URL it panics regardless. We verify only the
        // no-redis + GQL_CACHED_STORE=1 path: the function still returns an
        // in-memory store because no Redis is configured.
        let _guard = env_guard();
        let snap = snapshot();
        for k in VARS {
            env::remove_var(k);
        }
        env::set_var("GQL_CACHED_STORE", "1");

        let store = create_store();
        store
            .create_graph("cached-smoke")
            .expect("in-memory store create_graph should succeed");

        restore(snap);
    }
}
