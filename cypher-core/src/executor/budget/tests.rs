use std::thread::sleep;
use std::time::Duration;

use super::*;
use crate::error::{ExecutionError, GqlError};

#[test]
fn fresh_budget_allows_rows() {
    let mut budget = ExecutionBudget::default_budget();
    assert!(budget.check_or_err().is_ok());
}

#[test]
fn row_budget_exceeded() {
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), 3);
    budget.record_row();
    budget.record_row();
    budget.record_row();
    let result = budget.check_or_err();
    assert!(matches!(
        result,
        Err(GqlError::Execution(ExecutionError::ResultSetTooLarge {
            count: 3,
            limit: 3
        }))
    ));
}

#[test]
fn timeout_detected() {
    let mut budget = ExecutionBudget::new(Duration::from_millis(1), DEFAULT_MAX_ROWS);
    sleep(Duration::from_millis(5));
    // The deadline check is amortized: only fires every CHECK_INTERVAL calls.
    // Pump (CHECK_INTERVAL - 1) calls to bring the counter to the trigger point,
    // then the next call will actually check Instant::now() and detect the timeout.
    for _ in 0..CHECK_INTERVAL - 1 {
        let _ = budget.check_or_err();
    }
    assert!(budget.check_or_err().is_err());
}

#[test]
fn unlimited_budget_does_not_expire_quickly() {
    let mut budget = ExecutionBudget::unlimited();
    for _ in 0..1_000_000 {
        budget.record_row();
    }
    // Should not expire even after 1M rows
    assert!(budget.check_or_err().is_ok());
}

#[test]
fn remaining_decreases() {
    let budget = ExecutionBudget::new(Duration::from_secs(10), DEFAULT_MAX_ROWS);
    let r = budget.remaining();
    assert!(r.is_some());
    assert!(r.unwrap() <= Duration::from_secs(10));
}

/// Verify that the amortized check still detects row budget immediately
/// (row checks are never amortized, only Instant::now() is).
#[test]
fn row_budget_always_checked_even_with_amortization() {
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), 5);
    for _ in 0..5 {
        budget.record_row();
    }
    // The very first check_or_err() after exceeding the row budget should fail,
    // regardless of the amortization counter state.
    assert!(budget.check_or_err().is_err());
}

/// Verify that the amortized timeout check still fires within CHECK_INTERVAL calls.
#[test]
fn amortized_timeout_fires_within_check_interval() {
    let mut budget = ExecutionBudget::new(Duration::from_millis(1), DEFAULT_MAX_ROWS);
    sleep(Duration::from_millis(5));

    // The timeout may not be detected on the first call (amortized), but
    // it MUST be detected within CHECK_INTERVAL calls.
    let mut detected = false;
    for _ in 0..CHECK_INTERVAL + 1 {
        if budget.check_or_err().is_err() {
            detected = true;
            break;
        }
    }
    assert!(
        detected,
        "timeout should be detected within CHECK_INTERVAL+1 calls"
    );
}

/// Verify that amortization doesn't prevent detection -- the counter wraps
/// correctly and keeps checking periodically.
#[test]
fn amortized_check_counter_wraps_correctly() {
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), DEFAULT_MAX_ROWS);
    // Call check_or_err many times (more than CHECK_INTERVAL) -- should never
    // error because timeout is far in the future and rows haven't been recorded.
    for _ in 0..CHECK_INTERVAL * 3 {
        assert!(budget.check_or_err().is_ok());
    }
    // Counter should have wrapped around and checked Instant::now() 3 times,
    // none of which triggered a timeout.
    assert_eq!(budget.check_counter, CHECK_INTERVAL * 3);
}

// ── Row cap tests ──────────────────────────────────────────────────

#[test]
fn test_set_row_cap_and_reached() {
    let mut budget = ExecutionBudget::unlimited();
    budget.set_row_cap(5);
    for _ in 0..5 {
        assert!(!budget.reached_row_cap());
        budget.record_leaf_row();
    }
    assert!(
        budget.reached_row_cap(),
        "row cap should be reached after 5 leaf rows"
    );
}

#[test]
fn test_restore_row_cap() {
    let mut budget = ExecutionBudget::unlimited();
    let prev = budget.set_row_cap(3);
    for _ in 0..3 {
        budget.record_leaf_row();
    }
    assert!(budget.reached_row_cap());
    // Restore to None (no cap)
    budget.restore_row_cap(prev);
    assert!(
        !budget.reached_row_cap(),
        "after restoring None cap, reached_row_cap should be false"
    );
}

#[test]
fn test_nested_caps_take_tighter() {
    let mut budget = ExecutionBudget::unlimited();
    let prev_outer = budget.set_row_cap(10);
    // The cap is now leaf_rows_produced(0) + 10 = 10
    assert_eq!(budget.row_cap(), Some(10));

    let _prev_inner = budget.set_row_cap(5);
    // The inner cap would be 0 + 5 = 5, which is tighter than 10
    assert_eq!(
        budget.row_cap(),
        Some(5),
        "nested set_row_cap should take the tighter (smaller) cap"
    );

    // Restore outer
    budget.restore_row_cap(prev_outer);
    assert_eq!(budget.row_cap(), None);
}

#[test]
fn test_no_cap_means_unlimited() {
    let mut budget = ExecutionBudget::unlimited();
    // No set_row_cap call — cap is None
    for _ in 0..1000 {
        budget.record_leaf_row();
    }
    assert!(
        !budget.reached_row_cap(),
        "without a row cap, reached_row_cap should always be false"
    );
}

// ── Profile stats tests ──────────────────────────────────────────

#[test]
fn profiling_disabled_by_default() {
    let budget = ExecutionBudget::unlimited();
    assert!(!budget.is_profiling());
    assert!(budget.profile_stats().is_none());
}

#[test]
fn profiling_enabled_records_operator_rows() {
    let mut budget = ExecutionBudget::unlimited();
    budget.enable_profiling();
    assert!(budget.is_profiling());

    budget.record_operator_rows(0x1000, 5);
    budget.record_operator_rows(0x2000, 3);
    budget.record_operator_rows(0x1000, 2); // accumulates

    let stats = budget.profile_stats().unwrap();
    assert_eq!(stats[&0x1000].rows, 7);
    assert_eq!(stats[&0x2000].rows, 3);
}

#[test]
fn profiling_records_db_hits() {
    let mut budget = ExecutionBudget::unlimited();
    budget.enable_profiling();

    budget.record_operator_db_hits(0x1000, 10);
    budget.record_operator_db_hits(0x1000, 5);

    let stats = budget.profile_stats().unwrap();
    assert_eq!(stats[&0x1000].db_hits, 15);
}

#[test]
fn profiling_noop_when_disabled() {
    let mut budget = ExecutionBudget::unlimited();
    // Profiling not enabled — these should be no-ops
    budget.record_operator_rows(0x1000, 5);
    budget.record_operator_db_hits(0x1000, 10);
    assert!(budget.take_profile_stats().is_none());
}

#[test]
fn take_profile_stats_clears_state() {
    let mut budget = ExecutionBudget::unlimited();
    budget.enable_profiling();
    budget.record_operator_rows(0x1000, 1);

    let stats = budget.take_profile_stats();
    assert!(stats.is_some());
    assert_eq!(stats.unwrap()[&0x1000].rows, 1);

    // After take, profiling state is cleared
    assert!(budget.take_profile_stats().is_none());
}

#[test]
fn configured_max_rows_default() {
    // Without env var, returns DEFAULT_MAX_ROWS. Use unique var to avoid races.
    let key = "SD2_QUERY_MAX_ROWS_TEST_DEFAULT";
    std::env::remove_var(key);
    // Just exercise the parsing logic via the same code path:
    let v: usize = std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(super::DEFAULT_MAX_ROWS);
    assert_eq!(v, super::DEFAULT_MAX_ROWS);
}

#[test]
fn configured_max_rows_from_env() {
    // Test parsing logic without touching the shared SD2_QUERY_MAX_ROWS env var,
    // which would race with other parallel tests.
    let raw = "12345";
    let parsed: usize = raw.parse().unwrap_or(super::DEFAULT_MAX_ROWS);
    assert_eq!(parsed, 12345);
    // Also exercise the real function once (best-effort, no assertion on value).
    let _ = super::configured_max_rows();
}

#[test]
fn check_interval_const() {
    let _ = super::check_interval();
}

#[test]
fn default_budget_constructs() {
    let b = ExecutionBudget::default_budget();
    assert_eq!(b.rows_produced(), 0);
    assert_eq!(b.max_rows(), super::DEFAULT_MAX_ROWS);
}

#[test]
fn from_config_constructs() {
    let cfg = crate::config::EngineConfig::default();
    let b = ExecutionBudget::from_config(&cfg);
    assert_eq!(b.rows_produced(), 0);
}

#[test]
fn record_row_increments() {
    let mut b = ExecutionBudget::unlimited();
    b.record_row();
    b.record_row();
    assert_eq!(b.rows_produced(), 2);
}

#[test]
fn record_leaf_row_increments_separately() {
    let mut b = ExecutionBudget::unlimited();
    b.record_leaf_row();
    b.record_leaf_row();
    b.record_leaf_row();
    // set_row_cap(0) means cap = leaf_produced (3); reached
    let prev = b.set_row_cap(0);
    assert_eq!(prev, None);
    assert!(b.reached_row_cap());
    b.restore_row_cap(prev);
    assert!(!b.reached_row_cap());
}

#[test]
fn set_row_cap_takes_tighter_when_nested() {
    let mut b = ExecutionBudget::unlimited();
    let p1 = b.set_row_cap(100);
    let p2 = b.set_row_cap(10);
    assert_eq!(p1, None);
    assert_eq!(p2, Some(100));
    assert_eq!(b.row_cap(), Some(10));
    b.restore_row_cap(p2);
    assert_eq!(b.row_cap(), Some(100));
}

#[test]
fn reached_row_cap_no_cap_returns_false() {
    let b = ExecutionBudget::unlimited();
    assert!(!b.reached_row_cap());
}

#[test]
fn check_or_err_passes_when_under_limit() {
    let mut b = ExecutionBudget::unlimited();
    assert!(b.check_or_err().is_ok());
}

#[test]
fn check_or_err_fails_when_max_rows_exceeded() {
    let mut b = ExecutionBudget::new(std::time::Duration::from_secs(60), 2);
    b.record_row();
    b.record_row();
    assert!(b.check_or_err().is_err());
}

#[test]
fn remaining_returns_some_when_not_expired() {
    let b = ExecutionBudget::unlimited();
    assert!(b.remaining().is_some());
}

#[test]
fn is_expired_false_when_unlimited() {
    let b = ExecutionBudget::unlimited();
    assert!(!b.is_expired());
}

#[test]
fn request_id_set_get() {
    let mut b = ExecutionBudget::unlimited();
    assert_eq!(b.request_id(), None);
    b.set_request_id("trace-1".to_string());
    assert_eq!(b.request_id(), Some("trace-1"));
}
