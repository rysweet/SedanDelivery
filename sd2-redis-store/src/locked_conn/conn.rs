// `LockedRedisConn` connection wrapper holding one or more graph write locks.

use std::ops::{Deref, DerefMut};
use std::time::Instant;

use cypher_core::error::StorageError;

use crate::connection::RedisConn;
use crate::error_metrics::REDIS_WRITE_LOCK_HOLD_SECONDS;
use crate::lock;

use super::outcomes::{map_lock_release_result, map_lock_renewal_result};

pub(crate) struct HeldRedisLock {
    pub(crate) key: Box<str>,
    pub(crate) token: Box<str>,
}

pub(crate) struct LockedRedisConn {
    conn: RedisConn,
    graph: Box<str>,
    locks: Vec<HeldRedisLock>,
    ttl_ms: u64,
    acquired_at: Instant,
}

impl LockedRedisConn {
    pub(crate) fn new(
        conn: RedisConn,
        graph: &str,
        locks: Vec<(String, String)>,
        ttl_ms: u64,
    ) -> Self {
        Self {
            conn,
            graph: graph.into(),
            locks: locks
                .into_iter()
                .map(|(key, token)| HeldRedisLock {
                    key: key.into_boxed_str(),
                    token: token.into_boxed_str(),
                })
                .collect(),
            ttl_ms,
            acquired_at: Instant::now(),
        }
    }

    pub(crate) fn renew_lock(&mut self) -> Result<(), StorageError> {
        for held_lock in &self.locks {
            map_lock_renewal_result(
                self.graph.as_ref(),
                lock::renew(
                    self.conn.as_mut(),
                    held_lock.key.as_ref(),
                    held_lock.token.as_ref(),
                    self.ttl_ms,
                ),
            )?;
        }
        Ok(())
    }

    pub(crate) fn finish<T>(mut self, result: Result<T, StorageError>) -> Result<T, StorageError> {
        metrics::histogram!(REDIS_WRITE_LOCK_HOLD_SECONDS)
            .record(self.acquired_at.elapsed().as_secs_f64());
        let mut result = result;
        for held_lock in self.locks.iter().rev() {
            result = map_lock_release_result(
                self.graph.as_ref(),
                lock::release(
                    self.conn.as_mut(),
                    held_lock.key.as_ref(),
                    held_lock.token.as_ref(),
                ),
                result,
            );
        }
        result
    }
}

impl Deref for LockedRedisConn {
    type Target = RedisConn;

    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

impl DerefMut for LockedRedisConn {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.conn
    }
}
