// connection.rs — Connection pooling for standalone and Redis Cluster modes.

use std::time::Duration;

use redis::cluster::ClusterClient;
use redis::{Client, ConnectionLike};

use gql_core::error::StorageError;

/// Default connection pool size. Set high enough for concurrent bulk writes
/// (each Bolt session may hold a pool connection during query execution).
pub(crate) const DEFAULT_POOL_SIZE: u32 = 64;
/// Default pool checkout timeout in seconds.
pub(crate) const DEFAULT_POOL_TIMEOUT_SECS: u64 = 5;

/// Connection pool supporting both standalone and Redis Cluster modes.
///
/// Standalone mode uses a single Redis URL; cluster mode uses multiple seed
/// node URLs and auto-discovers the full cluster topology.
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
    ) -> Result<Self, StorageError> {
        let client = Client::open(url).map_err(|e| StorageError::Backend {
            msg: format!("redis client error: {e}"),
        })?;
        let pool = r2d2::Pool::builder()
            .max_size(pool_size)
            .connection_timeout(Duration::from_secs(pool_timeout_secs))
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
    ) -> Result<Self, StorageError> {
        let client = ClusterClient::new(nodes.iter().map(|s| s.to_string()).collect::<Vec<_>>())
            .map_err(|e| StorageError::Backend {
                msg: format!("redis cluster client error: {e}"),
            })?;
        let pool = r2d2::Pool::builder()
            .max_size(pool_size)
            .connection_timeout(Duration::from_secs(pool_timeout_secs))
            .build(client)
            .map_err(|e| StorageError::Backend {
                msg: format!("redis cluster pool error: {e}"),
            })?;
        Ok(RedisPool::Cluster(pool))
    }

    /// Check out a connection from the pool.
    pub(crate) fn get(&self) -> Result<RedisConn, StorageError> {
        match self {
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
        }
    }
}
