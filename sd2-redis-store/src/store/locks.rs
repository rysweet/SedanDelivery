// Closure-based write-lock orchestration on top of which all mutating
// operations in this crate are layered.

use std::time::Instant;

use cypher_core::error::StorageError;

use crate::error_metrics::{
    record_write_lock_backend_failure, REDIS_WRITE_LOCK_ACQUIRE_ATTEMPTS_TOTAL,
    REDIS_WRITE_LOCK_ACQUIRE_CONTENTION_TOTAL, REDIS_WRITE_LOCK_ACQUIRE_SUCCESSES_TOTAL,
    REDIS_WRITE_LOCK_ACQUIRE_WAIT_SECONDS, REDIS_WRITE_LOCK_CONTENTION_FAILURES_TOTAL,
};
use crate::keys::{key_graph_lock, key_graph_lock_fence};
use crate::lock;
use crate::locked_conn::{map_lock_release_result, LockedRedisConn};

use super::RedisGraphStore;

/// Release any locks accumulated during a partial write-lock acquisition,
/// folding each release outcome into the carried result so the caller sees
/// the first non-success error (and any cleanup-time failures fail the
/// chain even if the original outcome was `Ok`).
fn release_held(
    conn: &mut dyn redis::ConnectionLike,
    graph: &str,
    held: &mut Vec<(String, String)>,
    mut outcome: Result<(), StorageError>,
) -> Result<(), StorageError> {
    while let Some((held_key, held_token)) = held.pop() {
        outcome =
            map_lock_release_result(graph, lock::release(conn, &held_key, &held_token), outcome);
    }
    outcome
}

/// Outcome of acquiring the per-graph write lock plus any sub-locks.
struct AcquiredLocks {
    /// `(lock_key, token)` pairs in acquisition order.
    held: Vec<(String, String)>,
    /// Fence value bumped during the graph-lock acquisition (F1).
    fence: u64,
}

impl RedisGraphStore {
    fn try_acquire_write_locks(
        &self,
        conn: &mut dyn redis::ConnectionLike,
        graph: &str,
        graph_lock_key: &str,
        graph_fence_key: &str,
        sub_lock_keys: &[String],
        ttl_ms: u64,
    ) -> Result<Option<AcquiredLocks>, StorageError> {
        let mut held: Vec<(String, String)> = Vec::with_capacity(1 + sub_lock_keys.len());

        let fence = match lock::acquire_with_fence(conn, graph_lock_key, graph_fence_key, ttl_ms)? {
            Some((token, fence)) => {
                held.push((graph_lock_key.to_string(), token));
                fence
            }
            None => return Ok(None),
        };

        for lock_key in sub_lock_keys {
            match lock::acquire(conn, lock_key, ttl_ms) {
                Ok(Some(token)) => held.push((lock_key.clone(), token)),
                Ok(None) => {
                    release_held(conn, graph, &mut held, Ok(()))?;
                    return Ok(None);
                }
                Err(err) => {
                    return release_held(conn, graph, &mut held, Err(err))
                        .map(|_| unreachable!("cleanup after write-lock acquire failure"));
                }
            }
        }
        Ok(Some(AcquiredLocks { held, fence }))
    }

    /// Acquire the graph write lock plus the requested scoped lock (if any),
    /// run `f`, then release them in reverse order.
    ///
    /// This still serializes same-graph writes through the graph lock. The
    /// scoped key is an additional ownership marker for hot mutators, not a
    /// same-graph parallelism primitive.
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
        let graph_fence_key = self.kp(key_graph_lock_fence(graph));
        let sub_lock_keys: Vec<String> = if lock_key == graph_lock_key {
            Vec::new()
        } else {
            vec![lock_key]
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
            match self.try_acquire_write_locks(
                conn.as_mut(),
                graph,
                &graph_lock_key,
                &graph_fence_key,
                &sub_lock_keys,
                ttl_ms,
            ) {
                Ok(Some(acquired)) => {
                    if saw_contention {
                        metrics::counter!(REDIS_WRITE_LOCK_ACQUIRE_CONTENTION_TOTAL).increment(1);
                    }
                    metrics::counter!(REDIS_WRITE_LOCK_ACQUIRE_SUCCESSES_TOTAL).increment(1);
                    metrics::histogram!(REDIS_WRITE_LOCK_ACQUIRE_WAIT_SECONDS)
                        .record(lock_started_at.elapsed().as_secs_f64());
                    let mut locked_conn =
                        LockedRedisConn::new(conn, graph, acquired.held, acquired.fence, ttl_ms);
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
            let mut all_keys = vec![graph_lock_key.clone()];
            all_keys.extend(sub_lock_keys.iter().cloned());
            return Err(StorageError::Backend {
                msg: format!(
                    "write locks '{}' still contended after {max_retries} retries",
                    all_keys.join(", ")
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
}
