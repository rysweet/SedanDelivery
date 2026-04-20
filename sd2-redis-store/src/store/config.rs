// Constructors and configuration entry points for [`RedisGraphStore`].

use std::env;

use cypher_core::error::StorageError;
use cypher_core::storage::ReadConsistency;

use crate::connection::{
    RedisPool, ReplicaPool, DEFAULT_IDLE_TIMEOUT_SECS, DEFAULT_MIN_IDLE, DEFAULT_POOL_SIZE,
    DEFAULT_POOL_TIMEOUT_SECS,
};

use super::RedisGraphStore;

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
}

#[cfg(test)]
mod tests {
    //! Unit tests for `RedisGraphStore` constructors.
    //!
    //! Most paths build an `r2d2` pool and, when `min_idle >= 1`, eagerly
    //! connect to Redis. These tests use `min_idle = 0` via
    //! `from_url_with_options` so construction succeeds without a live
    //! server; the URL `redis://127.0.0.1:1` is unreachable but parses.
    //!
    //! `RedisGraphStore` does not implement `Debug`, so we use match-based
    //! unwrapping helpers instead of `.expect` / `.expect_err`.

    use super::*;
    use cypher_core::storage::ReadConsistency;

    const BOGUS_URL: &str = "redis://127.0.0.1:1";

    fn ok_store(r: Result<RedisGraphStore, StorageError>, ctx: &str) -> RedisGraphStore {
        match r {
            Ok(s) => s,
            Err(e) => panic!("{ctx}: expected Ok, got {e:?}"),
        }
    }

    fn err_store(r: Result<RedisGraphStore, StorageError>, ctx: &str) -> StorageError {
        match r {
            Err(e) => e,
            Ok(_) => panic!("{ctx}: expected Err, got Ok"),
        }
    }

    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        crate::test_env_lock::ENV_LOCK
            .lock()
            .expect("env lock should not be poisoned")
    }

    #[test]
    fn from_url_with_options_succeeds_when_min_idle_zero() {
        let store = ok_store(
            RedisGraphStore::from_url_with_options(BOGUS_URL, "tenant", 4, 1, 60, 0),
            "from_url_with_options",
        );
        assert_eq!(store.key_prefix, "tenant");
        assert!(store.replica_pool.is_none());
    }

    #[test]
    fn from_url_with_options_rejects_malformed_url() {
        let err = err_store(
            RedisGraphStore::from_url_with_options("not-a-redis-url", "", 1, 1, 60, 0),
            "malformed URL",
        );
        match err {
            StorageError::Backend { .. } => {}
            other => panic!("expected Backend error, got {other:?}"),
        }
    }

    #[test]
    fn from_url_with_options_propagates_empty_prefix() {
        let store = ok_store(
            RedisGraphStore::from_url_with_options(BOGUS_URL, "", 1, 1, 60, 0),
            "empty prefix",
        );
        assert!(store.key_prefix.is_empty());
    }

    #[test]
    fn from_url_with_prefix_delegates_to_default_options() {
        // DEFAULT_MIN_IDLE >= 1 so this attempts an eager connect against the
        // bogus URL and must fail with a backend error.
        let err = err_store(
            RedisGraphStore::from_url_with_prefix(BOGUS_URL, "tenant"),
            "from_url_with_prefix default min_idle",
        );
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn from_url_delegates_to_from_url_with_prefix() {
        let err = err_store(
            RedisGraphStore::from_url(BOGUS_URL),
            "from_url default min_idle",
        );
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn from_cluster_nodes_with_min_idle_zero_builds_pool() {
        let store = ok_store(
            RedisGraphStore::from_cluster_nodes(
                &[BOGUS_URL, "redis://127.0.0.1:2"],
                "c",
                4,
                1,
                60,
                0,
            ),
            "cluster construction",
        );
        assert_eq!(store.key_prefix, "c");
    }

    #[test]
    fn from_cluster_nodes_rejects_malformed_url() {
        let err = err_store(
            RedisGraphStore::from_cluster_nodes(&["not-a-redis-url"], "", 1, 1, 60, 0),
            "cluster malformed URL",
        );
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn with_read_consistency_mode_toggles_session_fresh() {
        let store = ok_store(
            RedisGraphStore::from_url_with_options(BOGUS_URL, "", 1, 1, 60, 0),
            "construction",
        );
        let pinned = store.with_read_consistency_mode(ReadConsistency::SessionFreshPrimary);
        assert!(matches!(
            pinned.read_consistency,
            ReadConsistency::SessionFreshPrimary
        ));
    }

    // ── from_env ────────────────────────────────────────────────────────────

    struct EnvRestorer<'a> {
        saved: Vec<(&'a str, Option<String>)>,
    }
    impl<'a> EnvRestorer<'a> {
        fn snapshot(vars: &'a [&'a str]) -> Self {
            let saved = vars.iter().map(|k| (*k, env::var(k).ok())).collect();
            Self { saved }
        }
    }
    impl<'a> Drop for EnvRestorer<'a> {
        fn drop(&mut self) {
            for (k, v) in &self.saved {
                match v {
                    Some(val) => env::set_var(k, val),
                    None => env::remove_var(k),
                }
            }
        }
    }

    const FROM_ENV_VARS: &[&str] = &[
        "GQL_REDIS_URL",
        "REDIS_URL",
        "GQL_REDIS_CLUSTER_NODES",
        "GQL_REDIS_REPLICA_URL",
        "GQL_REDIS_KEY_PREFIX",
        "GQL_REDIS_POOL_SIZE",
        "GQL_REDIS_POOL_TIMEOUT_SECS",
        "GQL_REDIS_IDLE_TIMEOUT_SECS",
        "GQL_REDIS_MIN_IDLE",
    ];

    fn clear_redis_env() {
        for k in FROM_ENV_VARS {
            env::remove_var(k);
        }
    }

    #[test]
    fn from_env_errors_when_no_url_configured() {
        let _guard = env_guard();
        let _restore = EnvRestorer::snapshot(FROM_ENV_VARS);
        clear_redis_env();

        let err = err_store(RedisGraphStore::from_env(), "no env config");
        match err {
            StorageError::Configuration { msg } => {
                assert!(msg.contains("GQL_REDIS_URL") || msg.contains("REDIS_URL"));
            }
            other => panic!("expected Configuration error, got {other:?}"),
        }
    }

    #[test]
    fn from_env_uses_standalone_url_with_zero_min_idle() {
        let _guard = env_guard();
        let _restore = EnvRestorer::snapshot(FROM_ENV_VARS);
        clear_redis_env();

        env::set_var("GQL_REDIS_URL", BOGUS_URL);
        env::set_var("GQL_REDIS_MIN_IDLE", "0");
        env::set_var("GQL_REDIS_POOL_SIZE", "2");
        env::set_var("GQL_REDIS_POOL_TIMEOUT_SECS", "1");
        env::set_var("GQL_REDIS_IDLE_TIMEOUT_SECS", "60");
        env::set_var("GQL_REDIS_KEY_PREFIX", "envprefix");

        let store = ok_store(RedisGraphStore::from_env(), "from_env standalone");
        assert_eq!(store.key_prefix, "envprefix");
    }

    #[test]
    fn from_env_falls_back_to_redis_url_when_gql_redis_url_unset() {
        let _guard = env_guard();
        let _restore = EnvRestorer::snapshot(FROM_ENV_VARS);
        clear_redis_env();

        env::set_var("REDIS_URL", BOGUS_URL);
        env::set_var("GQL_REDIS_MIN_IDLE", "0");
        env::set_var("GQL_REDIS_POOL_SIZE", "2");

        let store = ok_store(RedisGraphStore::from_env(), "REDIS_URL fallback");
        assert!(store.key_prefix.is_empty());
    }

    #[test]
    fn from_env_prefers_cluster_when_cluster_nodes_set() {
        let _guard = env_guard();
        let _restore = EnvRestorer::snapshot(FROM_ENV_VARS);
        clear_redis_env();

        env::set_var(
            "GQL_REDIS_CLUSTER_NODES",
            format!("{BOGUS_URL},redis://127.0.0.1:2"),
        );
        env::set_var("GQL_REDIS_MIN_IDLE", "0");
        env::set_var("GQL_REDIS_POOL_SIZE", "2");

        let store = ok_store(RedisGraphStore::from_env(), "from_env cluster");
        assert!(store.key_prefix.is_empty());
    }

    #[test]
    fn from_env_builds_replica_pool_when_replica_url_present() {
        let _guard = env_guard();
        let _restore = EnvRestorer::snapshot(FROM_ENV_VARS);
        clear_redis_env();

        env::set_var("GQL_REDIS_URL", BOGUS_URL);
        env::set_var(
            "GQL_REDIS_REPLICA_URL",
            "redis://127.0.0.1:3,redis://127.0.0.1:4",
        );
        env::set_var("GQL_REDIS_MIN_IDLE", "0");
        env::set_var("GQL_REDIS_POOL_SIZE", "4");

        let store = ok_store(RedisGraphStore::from_env(), "with replica pool");
        assert!(
            store.replica_pool.is_some(),
            "GQL_REDIS_REPLICA_URL must produce a replica pool"
        );
    }

    #[test]
    fn from_env_ignores_empty_replica_url_entries() {
        let _guard = env_guard();
        let _restore = EnvRestorer::snapshot(FROM_ENV_VARS);
        clear_redis_env();

        env::set_var("GQL_REDIS_URL", BOGUS_URL);
        env::set_var("GQL_REDIS_REPLICA_URL", ",,");
        env::set_var("GQL_REDIS_MIN_IDLE", "0");
        env::set_var("GQL_REDIS_POOL_SIZE", "2");

        let store = ok_store(RedisGraphStore::from_env(), "empty replica list");
        assert!(
            store.replica_pool.is_none(),
            "all-empty replica list must yield no replica pool"
        );
    }

    #[test]
    fn from_env_tolerates_bad_replica_url() {
        let _guard = env_guard();
        let _restore = EnvRestorer::snapshot(FROM_ENV_VARS);
        clear_redis_env();

        env::set_var("GQL_REDIS_URL", BOGUS_URL);
        env::set_var("GQL_REDIS_REPLICA_URL", "not-a-redis-url");
        env::set_var("GQL_REDIS_MIN_IDLE", "0");
        env::set_var("GQL_REDIS_POOL_SIZE", "2");

        // Bad replica URLs should log a warning and produce a store without a
        // replica pool rather than failing construction outright.
        let store = ok_store(RedisGraphStore::from_env(), "bad replica URL tolerated");
        assert!(store.replica_pool.is_none());
    }
}
