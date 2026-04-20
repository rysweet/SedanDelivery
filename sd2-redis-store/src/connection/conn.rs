// conn — Checked-out connection enum and cluster-aware pipeline wrapper.

use redis::cluster::ClusterPipeline;
use redis::{ConnectionLike, FromRedisValue, RedisResult, ToRedisArgs};

use super::manager::{TimeoutClientManager, TimeoutClusterClientManager};

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

#[cfg(test)]
mod tests {
    //! Tests for the cluster-aware pipeline wrapper. The `RedisConn` enum
    //! itself wraps `r2d2::PooledConnection`, which can only be constructed
    //! by checking out a connection from a real pool — so its inherent
    //! methods are exercised by the live-Redis suite under `tests/`.
    //!
    //! Here we cover both standalone and cluster variants of `RedisPipeline`
    //! through their builder methods, which never touch the network.

    use super::*;

    #[test]
    fn standalone_pipeline_cmd_arg_ignore_chain() {
        let mut p = RedisPipeline::Standalone(redis::pipe());
        // Each builder must return `&mut Self` for chaining.
        let _ = p.cmd("HSET").arg("k").arg(1u64).ignore();
        // Verify we still hold a Standalone variant after building.
        assert!(matches!(p, RedisPipeline::Standalone(_)));
    }

    #[test]
    fn cluster_pipeline_cmd_arg_ignore_chain() {
        let mut p = RedisPipeline::Cluster(redis::cluster::cluster_pipe());
        let _ = p.cmd("HSET").arg("k").arg(1u64).ignore();
        assert!(matches!(p, RedisPipeline::Cluster(_)));
    }
}
