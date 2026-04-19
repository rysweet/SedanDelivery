// Held write-lock state plus the connection wrapper that owns it.
//
// `LockedRedisConn` represents a connection on which one or more graph write
// locks are held. The closure-based `with_write_lock` API in `store.rs` hands
// out a `&mut LockedRedisConn` and ensures the locks are released (and the
// hold-time histogram is recorded) whether the closure returns Ok or Err.

use std::ops::{Deref, DerefMut};
use std::time::Instant;

use cypher_core::error::StorageError;

use crate::connection::RedisConn;
use crate::error_metrics::{
    record_write_lock_backend_failure, record_write_lock_conflict_outcome,
    REDIS_WRITE_LOCK_HOLD_SECONDS, REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL,
    REDIS_WRITE_LOCK_RELEASE_SUCCESSES_TOTAL, REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL,
    REDIS_WRITE_LOCK_RENEW_SUCCESSES_TOTAL,
};
use crate::lock;

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

pub(crate) fn map_lock_renewal_result(
    graph: &str,
    renewal: Result<bool, StorageError>,
) -> Result<(), StorageError> {
    match renewal {
        Ok(true) => {
            metrics::counter!(REDIS_WRITE_LOCK_RENEW_SUCCESSES_TOTAL).increment(1);
            Ok(())
        }
        Ok(false) => {
            metrics::counter!(REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL).increment(1);
            record_write_lock_conflict_outcome("renew");
            tracing::warn!(
                graph = %graph,
                "graph write lock expired or changed owner before renewal"
            );
            Err(StorageError::TransactionConflict)
        }
        Err(err) => {
            metrics::counter!(REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL).increment(1);
            record_write_lock_backend_failure("renew");
            tracing::warn!(
                graph = %graph,
                error = %err,
                "graph write lock renewal failed"
            );
            Err(err)
        }
    }
}

pub(crate) fn map_lock_release_result<T>(
    graph: &str,
    release: Result<bool, StorageError>,
    result: Result<T, StorageError>,
) -> Result<T, StorageError> {
    match release {
        Ok(true) => {
            metrics::counter!(REDIS_WRITE_LOCK_RELEASE_SUCCESSES_TOTAL).increment(1);
            result
        }
        Ok(false) => {
            metrics::counter!(REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL).increment(1);
            tracing::warn!(
                graph = %graph,
                "graph write lock expired or changed owner before release"
            );
            let result = match result {
                Ok(_) => Err(StorageError::TransactionConflict),
                Err(err) => Err(err),
            };
            if matches!(&result, Err(StorageError::TransactionConflict)) {
                record_write_lock_conflict_outcome("release");
            }
            result
        }
        Err(err) => {
            metrics::counter!(REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL).increment(1);
            record_write_lock_backend_failure("release");
            tracing::warn!(
                graph = %graph,
                error = %err,
                "graph write lock release failed"
            );
            match result {
                Ok(_) => Err(err),
                Err(work_err) => Err(work_err),
            }
        }
    }
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

#[cfg(test)]
mod write_lock_outcome_tests {
    use super::*;
    use crate::error_metrics::{
        REDIS_WRITE_LOCK_BACKEND_FAILURES_TOTAL, REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
    };
    use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
    use std::sync::{Mutex, OnceLock};

    fn metrics_handle() -> &'static PrometheusHandle {
        static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
        HANDLE.get_or_init(|| {
            PrometheusBuilder::new()
                .install_recorder()
                .expect("failed to install test Prometheus recorder")
        })
    }

    fn metrics_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock()
            .expect("metrics test lock should not be poisoned")
    }

    fn metric_value(handle: &PrometheusHandle, metric: &str, labels: &[(&str, &str)]) -> f64 {
        let output = handle.render();
        output
            .lines()
            .find_map(|line| {
                if line.starts_with('#') {
                    return None;
                }
                let rest = line.strip_prefix(metric)?;
                if !matches!(rest.chars().next(), Some(' ') | Some('{')) {
                    return None;
                }
                if labels.is_empty() && rest.starts_with('{') {
                    return None;
                }
                if !labels
                    .iter()
                    .all(|(key, value)| label_matches(line, key, value))
                {
                    return None;
                }
                line.split_whitespace().last()?.parse::<f64>().ok()
            })
            .unwrap_or(0.0)
    }

    fn label_matches(line: &str, key: &str, value: &str) -> bool {
        let expected = format!(r#"{key}="{value}""#);
        line.find(&expected).is_some_and(|index| {
            let bytes = line.as_bytes();
            let leading_ok = index == 0 || matches!(bytes.get(index - 1), Some(b'{') | Some(b','));
            let trailing_ok = matches!(bytes.get(index + expected.len()), Some(b',') | Some(b'}'));
            leading_ok && trailing_ok
        })
    }

    #[test]
    fn lost_lock_renewal_maps_to_transaction_conflict() {
        let _guard = metrics_guard();
        let result = map_lock_renewal_result("lease-loss", Ok(false));
        assert!(matches!(result, Err(StorageError::TransactionConflict)));
    }

    #[test]
    fn lost_lock_release_turns_successful_write_into_transaction_conflict() {
        let _guard = metrics_guard();
        let result = map_lock_release_result("lease-loss", Ok(false), Ok(()));
        assert!(matches!(result, Err(StorageError::TransactionConflict)));
    }

    #[test]
    fn lost_lock_release_preserves_underlying_write_error() {
        let _guard = metrics_guard();
        let result: Result<(), StorageError> = map_lock_release_result(
            "lease-loss",
            Ok(false),
            Err(StorageError::GraphNotFound {
                name: "missing".into(),
            }),
        );
        assert!(matches!(result, Err(StorageError::GraphNotFound { .. })));
    }

    #[test]
    fn release_error_after_successful_write_surfaces_release_error() {
        let _guard = metrics_guard();
        let result: Result<(), StorageError> = map_lock_release_result(
            "lease-loss",
            Err(StorageError::Backend {
                msg: "redis release failed".into(),
            }),
            Ok(()),
        );
        assert!(matches!(
            result,
            Err(StorageError::Backend { msg }) if msg == "redis release failed"
        ));
    }

    #[test]
    fn release_error_preserves_underlying_write_error() {
        let _guard = metrics_guard();
        let result: Result<(), StorageError> = map_lock_release_result(
            "lease-loss",
            Err(StorageError::Backend {
                msg: "redis release failed".into(),
            }),
            Err(StorageError::GraphNotFound {
                name: "missing".into(),
            }),
        );
        assert!(matches!(result, Err(StorageError::GraphNotFound { .. })));
    }

    #[test]
    fn renew_success_records_metric() {
        let _guard = metrics_guard();
        let handle = metrics_handle();
        let before = metric_value(handle, REDIS_WRITE_LOCK_RENEW_SUCCESSES_TOTAL, &[]);

        let result = map_lock_renewal_result("lease-loss", Ok(true));

        assert!(result.is_ok());
        let after = metric_value(handle, REDIS_WRITE_LOCK_RENEW_SUCCESSES_TOTAL, &[]);
        assert!(
            after >= before + 1.0,
            "renew success counter should increase (before={before}, after={after})"
        );
    }

    #[test]
    fn renew_conflict_records_failure_and_conflict_metrics() {
        let _guard = metrics_guard();
        let handle = metrics_handle();
        let before_failures = metric_value(handle, REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL, &[]);
        let before_conflicts = metric_value(
            handle,
            REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
            &[("stage", "renew")],
        );

        let result = map_lock_renewal_result("lease-loss", Ok(false));

        assert!(matches!(result, Err(StorageError::TransactionConflict)));
        let after_failures = metric_value(handle, REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL, &[]);
        let after_conflicts = metric_value(
            handle,
            REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
            &[("stage", "renew")],
        );
        assert!(after_failures >= before_failures + 1.0);
        assert!(after_conflicts >= before_conflicts + 1.0);
    }

    #[test]
    fn renew_backend_error_records_failure_and_backend_metric() {
        let _guard = metrics_guard();
        let handle = metrics_handle();
        let before_failures = metric_value(handle, REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL, &[]);
        let before_backend = metric_value(
            handle,
            REDIS_WRITE_LOCK_BACKEND_FAILURES_TOTAL,
            &[("stage", "renew")],
        );
        let before_conflicts = metric_value(
            handle,
            REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
            &[("stage", "renew")],
        );

        let result = map_lock_renewal_result(
            "lease-loss",
            Err(StorageError::Backend {
                msg: "redis renew failed".into(),
            }),
        );

        assert!(matches!(result, Err(StorageError::Backend { .. })));
        let after_failures = metric_value(handle, REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL, &[]);
        let after_backend = metric_value(
            handle,
            REDIS_WRITE_LOCK_BACKEND_FAILURES_TOTAL,
            &[("stage", "renew")],
        );
        let after_conflicts = metric_value(
            handle,
            REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
            &[("stage", "renew")],
        );
        assert!(after_failures >= before_failures + 1.0);
        assert!(after_backend >= before_backend + 1.0);
        assert_eq!(
            after_conflicts, before_conflicts,
            "backend errors during renew must not emit conflict outcomes"
        );
    }

    #[test]
    fn release_success_records_metric() {
        let _guard = metrics_guard();
        let handle = metrics_handle();
        let before = metric_value(handle, REDIS_WRITE_LOCK_RELEASE_SUCCESSES_TOTAL, &[]);

        let result: Result<(), StorageError> =
            map_lock_release_result("lease-loss", Ok(true), Ok(()));

        assert!(result.is_ok());
        let after = metric_value(handle, REDIS_WRITE_LOCK_RELEASE_SUCCESSES_TOTAL, &[]);
        assert!(
            after >= before + 1.0,
            "release success counter should increase (before={before}, after={after})"
        );
    }

    #[test]
    fn release_conflict_records_failure_and_conflict_metrics() {
        let _guard = metrics_guard();
        let handle = metrics_handle();
        let before_failures = metric_value(handle, REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL, &[]);
        let before_conflicts = metric_value(
            handle,
            REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
            &[("stage", "release")],
        );

        let result: Result<(), StorageError> =
            map_lock_release_result("lease-loss", Ok(false), Ok(()));

        assert!(matches!(result, Err(StorageError::TransactionConflict)));
        let after_failures = metric_value(handle, REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL, &[]);
        let after_conflicts = metric_value(
            handle,
            REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
            &[("stage", "release")],
        );
        assert!(after_failures >= before_failures + 1.0);
        assert!(after_conflicts >= before_conflicts + 1.0);
    }

    #[test]
    fn release_backend_error_records_backend_failure_metric() {
        let _guard = metrics_guard();
        let handle = metrics_handle();
        let before_failures = metric_value(handle, REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL, &[]);
        let before_backend = metric_value(
            handle,
            REDIS_WRITE_LOCK_BACKEND_FAILURES_TOTAL,
            &[("stage", "release")],
        );

        let result: Result<(), StorageError> = map_lock_release_result(
            "lease-loss",
            Err(StorageError::Backend {
                msg: "redis release failed".into(),
            }),
            Ok(()),
        );

        assert!(matches!(result, Err(StorageError::Backend { .. })));
        let after_failures = metric_value(handle, REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL, &[]);
        let after_backend = metric_value(
            handle,
            REDIS_WRITE_LOCK_BACKEND_FAILURES_TOTAL,
            &[("stage", "release")],
        );
        assert!(after_failures >= before_failures + 1.0);
        assert!(after_backend >= before_backend + 1.0);
    }

    #[test]
    fn release_conflict_with_failed_write_does_not_record_conflict_outcome() {
        let _guard = metrics_guard();
        let handle = metrics_handle();
        let before_failures = metric_value(handle, REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL, &[]);
        let before_conflicts = metric_value(
            handle,
            REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
            &[("stage", "release")],
        );

        let result: Result<(), StorageError> = map_lock_release_result(
            "lease-loss",
            Ok(false),
            Err(StorageError::GraphNotFound {
                name: "missing".into(),
            }),
        );

        assert!(matches!(result, Err(StorageError::GraphNotFound { .. })));
        let after_failures = metric_value(handle, REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL, &[]);
        let after_conflicts = metric_value(
            handle,
            REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
            &[("stage", "release")],
        );
        assert!(after_failures >= before_failures + 1.0);
        assert_eq!(
            after_conflicts, before_conflicts,
            "release conflict outcomes should not increment when the write had already failed"
        );
    }
}
