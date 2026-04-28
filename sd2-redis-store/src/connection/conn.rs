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
///
/// # Cross-slot pipelining guard (F5, issue #1769)
///
/// Hot mutating call sites are encouraged to record their key arguments via
/// [`RedisPipeline::key`] (which forwards to [`RedisPipeline::arg`] and also
/// remembers the key for a debug-time hash-tag uniformity check). When a
/// pipeline whose recorded keys span multiple Redis Cluster hash tags — or
/// which mixes a hash-tagged per-graph key with a non-tagged registry key —
/// is `query()`'d, `debug_assert!` will fire so cross-slot co-pipelining
/// cannot ship undetected. Untyped `.arg()` callers are grandfathered in
/// (the recorded-key list stays empty and the assertion trivially passes),
/// so this guard is opt-in for new code without forcing a sweep of every
/// existing call site.
pub(crate) enum RedisPipeline {
    Standalone(redis::Pipeline, KeyTagTracker),
    Cluster(ClusterPipeline, KeyTagTracker),
}

/// Records keys appended via [`RedisPipeline::key`] so [`RedisPipeline::query`]
/// can debug-assert that all keys in a single pipeline share the same Redis
/// Cluster hash tag (`{...}` substring).
#[derive(Default)]
pub(crate) struct KeyTagTracker {
    keys: Vec<String>,
}

impl KeyTagTracker {
    fn record(&mut self, key: &str) {
        self.keys.push(key.to_string());
    }

    /// Return `Err(reason)` if recorded keys do not all share the same hash
    /// tag, or if any recorded key is not hash-tagged at all (e.g. the graph
    /// registry key `__sd2_registry:graphs`).
    pub(crate) fn check_uniform_hash_tag(&self) -> Result<(), String> {
        if self.keys.is_empty() {
            return Ok(());
        }
        let first_tag = extract_hash_tag(&self.keys[0]);
        if first_tag.is_none() {
            return Err(format!(
                "pipeline contains a key without a `{{...}}` hash tag — \
                 this would route across slots on Redis Cluster: '{}'",
                self.keys[0]
            ));
        }
        for key in &self.keys[1..] {
            let tag = extract_hash_tag(key);
            if tag != first_tag {
                return Err(format!(
                    "pipeline mixes hash tags '{:?}' and '{:?}' — \
                     keys '{}' and '{}' would route to different cluster slots",
                    first_tag, tag, self.keys[0], key
                ));
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn keys(&self) -> &[String] {
        &self.keys
    }
}

/// Extract the substring between the first `{` and the first subsequent `}`,
/// matching Redis Cluster's hash-tag rule.
fn extract_hash_tag(key: &str) -> Option<&str> {
    let start = key.find('{')?;
    let rest = &key[start + 1..];
    let end = rest.find('}')?;
    if end == 0 {
        // `{}` is treated by Redis as no hash tag.
        return None;
    }
    Some(&rest[..end])
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
            RedisConn::Standalone(_) => {
                RedisPipeline::Standalone(redis::pipe(), KeyTagTracker::default())
            }
            RedisConn::Cluster(_) => {
                RedisPipeline::Cluster(redis::cluster::cluster_pipe(), KeyTagTracker::default())
            }
        }
    }
}

impl RedisPipeline {
    #[inline]
    pub(crate) fn cmd(&mut self, name: &str) -> &mut Self {
        match self {
            RedisPipeline::Standalone(pipe, _) => {
                pipe.cmd(name);
            }
            RedisPipeline::Cluster(pipe, _) => {
                pipe.cmd(name);
            }
        }
        self
    }

    #[inline]
    pub(crate) fn arg<T: ToRedisArgs>(&mut self, arg: T) -> &mut Self {
        match self {
            RedisPipeline::Standalone(pipe, _) => {
                pipe.arg(arg);
            }
            RedisPipeline::Cluster(pipe, _) => {
                pipe.arg(arg);
            }
        }
        self
    }

    /// Append a Redis key argument and record it for the cross-slot
    /// pipelining guard (F5). Equivalent to `arg(&key)` followed by adding
    /// `key` to the per-pipeline hash-tag tracker checked at `query()` time.
    ///
    /// Use this method (instead of `arg`) for every key argument of every
    /// command in a per-graph slot-local pipeline. Commands that legitimately
    /// take non-key value arguments (e.g. SADD member values, HSET field/
    /// value pairs) should keep using `arg`.
    #[inline]
    pub(crate) fn key(&mut self, key: impl Into<String>) -> &mut Self {
        let key = key.into();
        match self {
            RedisPipeline::Standalone(pipe, tracker) => {
                pipe.arg(&key);
                tracker.record(&key);
            }
            RedisPipeline::Cluster(pipe, tracker) => {
                pipe.arg(&key);
                tracker.record(&key);
            }
        }
        self
    }

    #[inline]
    pub(crate) fn ignore(&mut self) -> &mut Self {
        match self {
            RedisPipeline::Standalone(pipe, _) => {
                pipe.ignore();
            }
            RedisPipeline::Cluster(pipe, _) => {
                pipe.ignore();
            }
        }
        self
    }

    #[inline]
    pub(crate) fn query<T: FromRedisValue>(&self, conn: &mut RedisConn) -> RedisResult<T> {
        // Cross-slot co-pipelining guard (F5). In debug builds (and any test
        // configuration), assert that every recorded key shares the same
        // Redis Cluster hash tag. Untyped `arg(...)` callers do not record
        // anything, so this is a no-op for grandfathered code.
        if let Err(reason) = self.tracker().check_uniform_hash_tag() {
            debug_assert!(false, "cross-slot pipeline forbidden: {reason}");
            // In release builds, log loudly so a regression is visible in
            // production telemetry even though we don't panic.
            tracing::error!(target: "sd2_redis_store::pipeline", reason, "cross-slot pipeline detected");
        }
        match (self, conn) {
            (RedisPipeline::Standalone(pipe, _), RedisConn::Standalone(c)) => pipe.query(&mut **c),
            (RedisPipeline::Cluster(pipe, _), RedisConn::Cluster(c)) => pipe.query(c),
            _ => unreachable!("pipeline type must match connection type"),
        }
    }

    #[cfg(test)]
    pub(crate) fn tracker_mut(&mut self) -> &mut KeyTagTracker {
        match self {
            RedisPipeline::Standalone(_, t) => t,
            RedisPipeline::Cluster(_, t) => t,
        }
    }

    fn tracker(&self) -> &KeyTagTracker {
        match self {
            RedisPipeline::Standalone(_, t) => t,
            RedisPipeline::Cluster(_, t) => t,
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
        let mut p = RedisPipeline::Standalone(redis::pipe(), KeyTagTracker::default());
        // Each builder must return `&mut Self` for chaining.
        let _ = p.cmd("HSET").arg("k").arg(1u64).ignore();
        // Verify we still hold a Standalone variant after building.
        assert!(matches!(p, RedisPipeline::Standalone(..)));
    }

    #[test]
    fn cluster_pipeline_cmd_arg_ignore_chain() {
        let mut p =
            RedisPipeline::Cluster(redis::cluster::cluster_pipe(), KeyTagTracker::default());
        let _ = p.cmd("HSET").arg("k").arg(1u64).ignore();
        assert!(matches!(p, RedisPipeline::Cluster(..)));
    }

    // ── F5: cross-slot pipelining guard ──────────────────────────────────

    #[test]
    fn key_tracker_records_keys_via_key_method() {
        let mut p = RedisPipeline::Standalone(redis::pipe(), KeyTagTracker::default());
        let _ = p.cmd("SADD").key("G:{g1}:nodes").arg("1").ignore();
        let _ = p.cmd("SADD").key("G:{g1}:label:Person").arg("1").ignore();
        let recorded: Vec<String> = p.tracker_mut().keys().to_vec();
        assert_eq!(
            recorded,
            vec![
                "G:{g1}:nodes".to_string(),
                "G:{g1}:label:Person".to_string()
            ],
            "the .key() builder method must remember every key it appends so the \
             cross-slot pipelining guard can verify hash-tag uniformity at query time"
        );
    }

    #[test]
    fn uniform_hash_tag_passes_for_same_graph_keys() {
        let mut tracker = KeyTagTracker::default();
        tracker.record("G:{db1}:nodes");
        tracker.record("G:{db1}:label:Person");
        tracker.record("G:{db1}:node:42");
        tracker
            .check_uniform_hash_tag()
            .expect("same-graph keys must pass the uniformity check");
    }

    #[test]
    fn uniform_hash_tag_fails_when_keys_span_two_graphs() {
        let mut tracker = KeyTagTracker::default();
        tracker.record("G:{db1}:nodes");
        tracker.record("G:{db2}:nodes");
        let err = tracker
            .check_uniform_hash_tag()
            .expect_err("mixed-tag pipeline must be rejected");
        assert!(
            err.contains("db1") && err.contains("db2"),
            "error must name the offending tags: {err}"
        );
    }

    #[test]
    fn uniform_hash_tag_fails_when_registry_key_mixed_in_per_graph_pipeline() {
        let mut tracker = KeyTagTracker::default();
        tracker.record("G:{db1}:nodes");
        tracker.record("__sd2_registry:graphs");
        let err = tracker
            .check_uniform_hash_tag()
            .expect_err("registry key has no hash tag and must be rejected");
        assert!(
            err.contains("__sd2_registry:graphs"),
            "error must surface the offending non-tagged key: {err}"
        );
    }

    #[test]
    fn empty_tracker_is_trivially_uniform() {
        let tracker = KeyTagTracker::default();
        tracker
            .check_uniform_hash_tag()
            .expect("empty trackers (legacy arg-only callers) must not regress");
    }

    #[test]
    fn extract_hash_tag_returns_none_for_empty_braces() {
        assert!(extract_hash_tag("foo:{}:bar").is_none());
    }

    #[test]
    fn extract_hash_tag_returns_first_brace_pair() {
        assert_eq!(extract_hash_tag("G:{db1}:foo"), Some("db1"));
        assert_eq!(extract_hash_tag("a:{x}b{y}"), Some("x"));
    }
}
