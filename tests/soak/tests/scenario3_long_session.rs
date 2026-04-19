//! Scenario 3 — long-running session.
//!
//! Single embedded Cypher session, 1M total queries. Verify no panics,
//! no error escalation, and that p99 latency does not degrade by more
//! than 3x between the first and last 10% of the run.

use std::time::{Duration, Instant};

use sedan_cypher_embedded::CypherEngine;
use soak::{build_person_graph, scaled_iters, PercentileTracker, TimeoutGuard, READ_QUERIES};

const NODES: usize = 5_000;
/// Spec target: 1M iterations. Scaled by `SOAK_ITER_SCALE` (default 1.0).
const ITERS_BASE: usize = 1_000_000;
const LATENCY_BUDGET: f64 = 3.0;

#[test]
#[ignore = "soak — opt-in via cargo test -p soak -- --ignored"]
fn scenario3_long_running_session() {
    let _guard = TimeoutGuard::arm("scenario3", Duration::from_secs(2400));

    let iters = scaled_iters(ITERS_BASE);
    let window = (iters / 10).max(1);

    let store = build_person_graph(NODES);
    let engine = CypherEngine::new();

    let mut early = PercentileTracker::new();
    let mut late = PercentileTracker::new();
    let mut errors: u64 = 0;

    for i in 0..iters {
        let q = READ_QUERIES[i % READ_QUERIES.len()];
        let t0 = Instant::now();
        let result = engine.execute(&store, soak::GRAPH, q);
        let dt = t0.elapsed();

        if result.is_err() {
            errors += 1;
            // Bail early if errors escalate — that's a regression itself.
            assert!(
                errors < 100,
                "long-running session error escalation at iter {i}: {} errors",
                errors
            );
        }

        if i < window {
            early.record(dt);
        } else if i >= iters - window {
            late.record(dt);
        }

        if i % 100_000 == 0 && i > 0 {
            println!("[soak/scenario3] iter={i} errors={errors}");
        }
    }

    let early_p99 = early.percentile_micros(0.99);
    let late_p99 = late.percentile_micros(0.99);

    let ratio = if early_p99 == 0 {
        0.0
    } else {
        (late_p99 as f64) / (early_p99 as f64)
    };

    println!(
        "[soak/scenario3] iter={iters} errors={errors} early_p99_us={early_p99} \
         late_p99_us={late_p99} ratio={ratio:.2}x budget={LATENCY_BUDGET}x {}",
        if ratio < LATENCY_BUDGET {
            "pass"
        } else {
            "FAIL"
        }
    );

    assert_eq!(errors, 0, "long-running session produced {errors} errors");
    assert!(
        ratio < LATENCY_BUDGET,
        "p99 latency degraded {ratio:.2}x (>= {LATENCY_BUDGET}x budget): \
         early_p99={early_p99} us, late_p99={late_p99} us"
    );
}
