// Backend-error counter and write-lock metric constants/helpers.
//
// Centralizes the cross-cutting metrics names and the backend-error-message
// sanitization helpers used by every storage operation in this crate.

use std::sync::atomic::{AtomicU64, Ordering};

use cypher_core::error::StorageError;

// ── Backend-error counter ────────────────────────────────────────────────────

/// Global count of `StorageError::Backend` occurrences.
static BACKEND_ERROR_COUNT: AtomicU64 = AtomicU64::new(0);

// ── Write-lock metrics names ─────────────────────────────────────────────────

pub(crate) const REDIS_WRITE_LOCK_ACQUIRE_ATTEMPTS_TOTAL: &str =
    "redis_write_lock_acquire_attempts_total";
pub(crate) const REDIS_WRITE_LOCK_ACQUIRE_SUCCESSES_TOTAL: &str =
    "redis_write_lock_acquire_successes_total";
pub(crate) const REDIS_WRITE_LOCK_ACQUIRE_CONTENTION_TOTAL: &str =
    "redis_write_lock_acquire_contention_total";
pub(crate) const REDIS_WRITE_LOCK_CONTENTION_FAILURES_TOTAL: &str =
    "redis_write_lock_contention_failures_total";
pub(crate) const REDIS_WRITE_LOCK_ACQUIRE_WAIT_SECONDS: &str =
    "redis_write_lock_acquire_wait_seconds";
pub(crate) const REDIS_WRITE_LOCK_RENEW_SUCCESSES_TOTAL: &str =
    "redis_write_lock_renew_successes_total";
pub(crate) const REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL: &str =
    "redis_write_lock_renew_failures_total";
pub(crate) const REDIS_WRITE_LOCK_RELEASE_SUCCESSES_TOTAL: &str =
    "redis_write_lock_release_successes_total";
pub(crate) const REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL: &str =
    "redis_write_lock_release_failures_total";
pub(crate) const REDIS_WRITE_LOCK_HOLD_SECONDS: &str = "redis_write_lock_hold_seconds";
pub(crate) const REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL: &str =
    "redis_write_lock_conflict_outcomes_total";
pub(crate) const REDIS_WRITE_LOCK_BACKEND_FAILURES_TOTAL: &str =
    "redis_write_lock_backend_failures_total";

/// Increment the `StorageError::Backend` counter by one.
pub fn record_backend_error() {
    BACKEND_ERROR_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Return the current `StorageError::Backend` count.
pub fn backend_error_count() -> u64 {
    BACKEND_ERROR_COUNT.load(Ordering::Relaxed)
}

pub(crate) fn record_write_lock_conflict_outcome(stage: &'static str) {
    metrics::counter!(REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL, "stage" => stage).increment(1);
}

pub(crate) fn record_write_lock_backend_failure(stage: &'static str) {
    metrics::counter!(REDIS_WRITE_LOCK_BACKEND_FAILURES_TOTAL, "stage" => stage).increment(1);
}

/// Sanitize a backend error message for inclusion in `StorageError::Backend`.
///
/// In debug builds the full message is returned so engineers can see the
/// underlying Redis error detail.  In release builds the message is replaced
/// with a fixed string so internal Redis topology information (e.g. MOVED
/// redirects with cluster node IPs) is not leaked to callers (SEC-6).
#[cfg(debug_assertions)]
pub fn backend_error_msg(msg: &str) -> String {
    msg.to_string()
}

/// Sanitize a backend error message for inclusion in `StorageError::Backend`.
///
/// In release builds the message is replaced with a fixed string so internal
/// Redis topology information is not leaked to callers (SEC-6).
#[cfg(not(debug_assertions))]
pub fn backend_error_msg(_msg: &str) -> String {
    "backend error".to_string()
}

pub(crate) fn backend_storage_error<E: std::fmt::Display>(e: E) -> StorageError {
    record_backend_error();
    let msg = e.to_string();
    StorageError::Backend {
        msg: backend_error_msg(&msg),
    }
}

#[cfg(test)]
mod backend_error_counter_tests {
    use super::*;

    #[test]
    fn backend_error_count_starts_at_zero_or_valid() {
        // Note: counter may be > 0 if other tests ran first.
        // Just verify it's a valid u64.
        let _ = backend_error_count();
    }

    #[test]
    fn record_backend_error_increments_counter() {
        let before = backend_error_count();
        record_backend_error();
        let after = backend_error_count();
        assert_eq!(after, before + 1, "counter should increment by 1");
    }
}
