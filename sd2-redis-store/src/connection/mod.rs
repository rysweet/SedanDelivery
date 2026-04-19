// connection — Connection pooling for standalone and Redis Cluster modes.
//
// Decomposed into focused submodules:
// * `config`   — pool-size / timeout constants, env-var helpers, [`PoolStats`].
// * `manager`  — `r2d2::ManageConnection` adapters that apply socket timeouts.
// * `conn`     — checked-out connection enum and cluster-aware pipeline.
// * `pool`     — primary [`RedisPool`] supporting standalone + cluster modes.
// * `replica`  — round-robin [`ReplicaPool`] for read replicas.

mod config;
mod conn;
mod manager;
mod pool;
mod replica;

pub use config::PoolStats;
pub(crate) use config::{
    DEFAULT_IDLE_TIMEOUT_SECS, DEFAULT_MIN_IDLE, DEFAULT_POOL_SIZE, DEFAULT_POOL_TIMEOUT_SECS,
};
pub(crate) use conn::RedisConn;
pub(crate) use pool::RedisPool;
pub(crate) use replica::ReplicaPool;
