//! Scenario 1 — steady-state query soak.
//!
//! Build a 10K-node graph, then loop 100K iterations executing a mix of
//! read queries through the embedded Cypher engine. Sample RSS every 1K
//! iterations. Assert that RSS at iter 100K is < 1.5x RSS at iter 10K.

use std::time::Duration;

use sedan_cypher_embedded::CypherEngine;
use soak::{
    build_person_graph, count_nodes, rss_growth_pct, rss_kb, scaled_iters, TimeoutGuard,
    READ_QUERIES,
};

const NODES: usize = 10_000;
/// Spec target: 100K iterations. Scaled by `SOAK_ITER_SCALE` (default 1.0).
/// Defaults are sized so the full soak script fits in a 30-minute budget on
/// typical CI runners (~3-13 ms per query × four-query mix).
const ITERS_BASE: usize = 100_000;
const LEAK_BUDGET: f64 = 1.5; // RSS_end < 1.5 * RSS_baseline

#[test]
#[ignore = "soak — opt-in via cargo test -p soak -- --ignored"]
fn scenario1_steady_state_query_soak() {
    let _guard = TimeoutGuard::arm("scenario1", Duration::from_secs(1500));

    let iters = scaled_iters(ITERS_BASE);
    // Sample at most ~100 ticks across the run.
    let sample_every = (iters / 100).max(1);
    // Baseline at 10% in (matches the spec's "iter 10K of 100K"). Snap to
    // the sample_every grid so the equality check below fires.
    let baseline_at = ((iters / 10) / sample_every).max(1) * sample_every;

    let store = build_person_graph(NODES);
    let engine = CypherEngine::new();

    // Sanity check that the graph is intact before we start measuring.
    let initial_count = count_nodes(&engine, &store);
    assert_eq!(initial_count, NODES as i64);

    let mut baseline_rss: u64 = 0;
    let mut last_rss: u64 = 0;

    for i in 0..iters {
        let q = READ_QUERIES[i % READ_QUERIES.len()];
        let _ = engine.execute(&store, soak::GRAPH, q).expect("query");

        if i % sample_every == 0 {
            let rss = rss_kb();
            if i == baseline_at {
                baseline_rss = rss;
            }
            last_rss = rss;
            if i % (sample_every * 10).max(1) == 0 {
                println!("[soak/scenario1] iter={i} rss_kb={rss} baseline_kb={baseline_rss}");
            }
        }
    }

    // Final integrity check — graph must be unchanged after read-only soak.
    assert_eq!(count_nodes(&engine, &store), NODES as i64);

    if baseline_rss == 0 {
        println!(
            "[soak/scenario1] iter={iters} rss_growth=unknown (RSS unavailable on this platform) pass"
        );
        return;
    }

    let growth_pct = rss_growth_pct(baseline_rss, last_rss);
    let ratio = (last_rss as f64) / (baseline_rss as f64);
    println!(
        "[soak/scenario1] iter={iters} rss_baseline_kb={baseline_rss} rss_final_kb={last_rss} \
         rss_growth={growth_pct:.1}% ratio={ratio:.2}x budget={LEAK_BUDGET}x {}",
        if ratio < LEAK_BUDGET { "pass" } else { "FAIL" }
    );

    assert!(
        ratio < LEAK_BUDGET,
        "RSS grew {ratio:.2}x (>= {LEAK_BUDGET}x leak budget): \
         baseline={baseline_rss} kB, final={last_rss} kB"
    );
}
