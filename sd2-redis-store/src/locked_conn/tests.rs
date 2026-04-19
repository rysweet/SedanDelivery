use super::outcomes::{map_lock_release_result, map_lock_renewal_result};
use crate::error_metrics::{
    REDIS_WRITE_LOCK_BACKEND_FAILURES_TOTAL, REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
    REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL, REDIS_WRITE_LOCK_RELEASE_SUCCESSES_TOTAL,
    REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL, REDIS_WRITE_LOCK_RENEW_SUCCESSES_TOTAL,
};
use cypher_core::error::StorageError;
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

    let result: Result<(), StorageError> = map_lock_release_result("lease-loss", Ok(true), Ok(()));

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

    let result: Result<(), StorageError> = map_lock_release_result("lease-loss", Ok(false), Ok(()));

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
