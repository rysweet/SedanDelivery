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
