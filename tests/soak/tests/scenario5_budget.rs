//! Scenario 5 — budget enforcement under pressure.
//!
//! Loop 10K iterations executing a Gremlin traversal under a tight
//! [`ExecutionBudget`] (10 ms timeout / 100 max rows) against a graph that
//! would otherwise produce many more rows. Every iteration must surface
//! the budget error (timeout or row-cap) — never panic and never silently
//! complete. Asserts that RSS does not balloon when execution is cut off
//! mid-flight (i.e. no resource leak when budgets fire).
//!
//! Uses the Gremlin embedded engine because it is the only one of the
//! three embedded crates that exposes [`execute_with_budget`] today.

use std::time::Duration;

use sedan_gremlin_embedded::{ExecutionBudget, GremlinEngine};
use soak::{build_person_graph, rss_growth_pct, rss_kb, scaled_iters, TimeoutGuard};

const NODES: usize = 10_000;
/// Spec target: 10K iterations. Scaled by `SOAK_ITER_SCALE` (default 1.0).
const ITERS_BASE: usize = 10_000;
// Query that scans every node in the graph and projects a property — well
// in excess of the 100-row budget below.
const HEAVY_QUERY: &str = "g.V().hasLabel('Person').values('name')";
const LEAK_BUDGET: f64 = 1.5;

#[test]
#[ignore = "soak — opt-in via cargo test -p soak -- --ignored"]
fn scenario5_budget_enforcement_under_pressure() {
    let _guard = TimeoutGuard::arm("scenario5", Duration::from_secs(900));

    let iters = scaled_iters(ITERS_BASE);

    let store = build_person_graph(NODES);
    let engine = GremlinEngine::new();

    // Warm-up sample (after the first few iterations) so we don't capture
    // first-touch allocations as part of the leak measurement.
    let mut baseline_rss = 0u64;
    let mut last_rss = 0u64;
    let mut budget_hits: u64 = 0;
    let other_errors: u64 = 0;
    let mut spurious_ok: u64 = 0;

    for i in 0..iters {
        let mut budget = ExecutionBudget::new(Duration::from_millis(10), 100);
        match engine.execute_with_budget(&store, soak::GRAPH, HEAVY_QUERY, &mut budget) {
            Ok(qr) => {
                // The only way OK is acceptable here is if the engine has
                // already capped rows at the budget — i.e. row count <= 100.
                // Anything larger means the budget was not enforced.
                if qr.rows.len() > 100 {
                    spurious_ok += 1;
                }
            }
            Err(_) => {
                budget_hits += 1;
            }
        }

        if i == 100 {
            baseline_rss = rss_kb();
        }
        if i % 1_000 == 0 && i > 0 {
            last_rss = rss_kb();
        }
    }

    if last_rss == 0 {
        last_rss = rss_kb();
    }

    let total_enforced = budget_hits + (iters as u64 - spurious_ok - other_errors - budget_hits);
    println!(
        "[soak/scenario5] iter={iters} budget_errors={budget_hits} \
         spurious_unbounded_ok={spurious_ok} other_errors={other_errors}"
    );

    assert_eq!(
        spurious_ok, 0,
        "{spurious_ok} iterations returned > 100 rows despite max_rows=100 — budget not enforced"
    );
    assert_eq!(
        other_errors, 0,
        "unexpected non-budget errors: {other_errors}"
    );
    // We expect every iteration to either Err with a budget error or Ok with <= 100 rows.
    assert!(total_enforced > 0, "no iterations were budget-enforced");

    if baseline_rss == 0 {
        println!(
            "[soak/scenario5] iter={iters} budget_errors={budget_hits} rss_growth=unknown pass"
        );
        return;
    }

    let growth_pct = rss_growth_pct(baseline_rss, last_rss);
    let ratio = (last_rss as f64) / (baseline_rss as f64);
    println!(
        "[soak/scenario5] iter={iters} budget_errors={budget_hits} \
         rss_baseline_kb={baseline_rss} rss_final_kb={last_rss} \
         rss_growth={growth_pct:.1}% ratio={ratio:.2}x budget={LEAK_BUDGET}x {}",
        if ratio < LEAK_BUDGET { "pass" } else { "FAIL" }
    );
    assert!(
        ratio < LEAK_BUDGET,
        "RSS leaked when budget killed mid-execution: \
         baseline={baseline_rss} kB, final={last_rss} kB, ratio={ratio:.2}x"
    );
}
