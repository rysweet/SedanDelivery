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
/// Counter incremented when a Lua-atomic hot-write script rejects a write
/// because the per-graph fence has advanced past the writer's expected token
/// (i.e. the writer's lease was stolen and a newer writer has taken over).
///
/// **Not** classified as a backend error — fence rejection is the system
/// behaving correctly under TTL-driven lease takeover, so the
/// resharding-resilience smoke test that checks for `*_backend_errors_*`
/// series will not flag this counter.
pub(crate) const REDIS_LOCK_FENCE_REJECTED_TOTAL: &str = "sd2_redis_lock_fence_rejected_total";

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

/// Increment the fence-rejection counter, labelled with the graph that
/// triggered the rejection (F1 in the cluster consistency hardening package).
pub(crate) fn record_lock_fence_rejected(graph: &str) {
    metrics::counter!(REDIS_LOCK_FENCE_REJECTED_TOTAL, "graph" => graph.to_string()).increment(1);
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

    // ── backend_error_msg (debug build: preserves message) ───────────────
    #[test]
    #[cfg(debug_assertions)]
    fn backend_error_msg_preserves_in_debug() {
        let out = backend_error_msg("MOVED 1234 1.2.3.4:7001");
        assert_eq!(out, "MOVED 1234 1.2.3.4:7001");
    }

    #[test]
    fn backend_error_msg_is_non_empty() {
        let out = backend_error_msg("any");
        assert!(!out.is_empty());
    }

    #[test]
    fn backend_storage_error_produces_backend_variant_and_increments() {
        let before = backend_error_count();
        let e = backend_storage_error("boom");
        let after = backend_error_count();
        assert!(after > before);
        match e {
            StorageError::Backend { .. } => {}
            other => panic!("expected Backend, got {other:?}"),
        }
    }

    #[test]
    fn record_write_lock_conflict_outcome_is_callable() {
        // The metrics crate uses a global recorder; without one installed,
        // these calls should be no-ops that still exercise the code path.
        record_write_lock_conflict_outcome("acquire");
        record_write_lock_conflict_outcome("renew");
    }

    #[test]
    fn record_write_lock_backend_failure_is_callable() {
        record_write_lock_backend_failure("acquire");
        record_write_lock_backend_failure("release");
    }

    #[test]
    fn metric_name_constants_are_distinct_and_prefixed() {
        let names = [
            REDIS_WRITE_LOCK_ACQUIRE_ATTEMPTS_TOTAL,
            REDIS_WRITE_LOCK_ACQUIRE_SUCCESSES_TOTAL,
            REDIS_WRITE_LOCK_ACQUIRE_CONTENTION_TOTAL,
            REDIS_WRITE_LOCK_CONTENTION_FAILURES_TOTAL,
            REDIS_WRITE_LOCK_ACQUIRE_WAIT_SECONDS,
            REDIS_WRITE_LOCK_RENEW_SUCCESSES_TOTAL,
            REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL,
            REDIS_WRITE_LOCK_RELEASE_SUCCESSES_TOTAL,
            REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL,
            REDIS_WRITE_LOCK_HOLD_SECONDS,
            REDIS_WRITE_LOCK_CONFLICT_OUTCOMES_TOTAL,
            REDIS_WRITE_LOCK_BACKEND_FAILURES_TOTAL,
        ];
        for n in names {
            assert!(n.starts_with("redis_write_lock_"), "got {n}");
        }
        // All distinct.
        let mut sorted: Vec<&&str> = names.iter().collect();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
    }
}
