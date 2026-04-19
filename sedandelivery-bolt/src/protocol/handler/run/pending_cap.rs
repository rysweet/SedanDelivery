//! Pending-results cap configuration.
//!
//! Caps the number of `PendingQuery` entries a session may hold concurrently
//! to prevent OOM from misbehaving clients that issue many RUNs without
//! consuming results via PULL/DISCARD. Configurable via the
//! `BOLT_MAX_PENDING_RESULTS` environment variable.

const DEFAULT_MAX_PENDING_RESULTS: usize = 16;

pub(super) fn max_pending_results() -> usize {
    static CAP: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *CAP.get_or_init(|| {
        std::env::var("BOLT_MAX_PENDING_RESULTS")
            .ok()
            .and_then(|v| match v.parse() {
                Ok(val) => Some(val),
                Err(_) => {
                    tracing::warn!(
                        "invalid value for BOLT_MAX_PENDING_RESULTS: {v:?}, using default {DEFAULT_MAX_PENDING_RESULTS}"
                    );
                    None
                }
            })
            .unwrap_or(DEFAULT_MAX_PENDING_RESULTS)
    })
}
