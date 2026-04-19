//! Scenario 2 — insert/delete churn.
//!
//! Loop 50K iterations of CREATE then DELETE pairs against an embedded
//! Cypher engine. Sample node count + RSS periodically. Assert that the
//! final node count equals the initial node count and RSS stays within
//! the leak budget.

use std::time::Duration;

use sedan_cypher_embedded::CypherEngine;
use soak::{build_person_graph, count_nodes, rss_growth_pct, rss_kb, scaled_iters, TimeoutGuard};

const NODES: usize = 1_000;
/// Spec target: 50K iterations. Scaled by `SOAK_ITER_SCALE` (default 1.0).
const ITERS_BASE: usize = 50_000;
const LEAK_BUDGET: f64 = 1.5;

#[test]
#[ignore = "soak — opt-in via cargo test -p soak -- --ignored"]
fn scenario2_insert_delete_churn() {
    let _guard = TimeoutGuard::arm("scenario2", Duration::from_secs(1500));

    let iters = scaled_iters(ITERS_BASE);
    // Sample at most ~20 ticks across the run — keeps the test cheap at
    // any scale and ensures `baseline_at` is always reached.
    let sample_every = (iters / 20).max(1);
    let baseline_at = sample_every; // first non-zero sample tick

    let store = build_person_graph(NODES);
    let engine = CypherEngine::new();

    let initial_count = count_nodes(&engine, &store);
    assert_eq!(initial_count, NODES as i64);

    let mut baseline_rss = 0u64;
    let mut last_rss = 0u64;

    for i in 0..iters {
        // Use a unique tag so the matching DELETE only removes what we created.
        let tag = format!("soak2_{i}");
        engine
            .execute(
                &store,
                soak::GRAPH,
                &format!("CREATE (:Tmp {{tag: '{tag}', i: {i}}})"),
            )
            .expect("create");
        engine
            .execute(
                &store,
                soak::GRAPH,
                &format!("MATCH (n:Tmp {{tag: '{tag}'}}) DELETE n"),
            )
            .expect("delete");

        if i % sample_every == 0 {
            let rss = rss_kb();
            let count = count_nodes(&engine, &store);
            if i == baseline_at {
                baseline_rss = rss;
            }
            last_rss = rss;
            println!("[soak/scenario2] iter={i} nodes={count} rss_kb={rss}");
            // Node count must always be the initial value at sampling boundaries.
            assert_eq!(
                count, initial_count,
                "node count drifted at iter {i}: expected {initial_count}, got {count}"
            );
        }
    }

    let final_count = count_nodes(&engine, &store);
    assert_eq!(
        final_count, initial_count,
        "node count drifted: initial={initial_count}, final={final_count}"
    );

    if baseline_rss == 0 {
        println!("[soak/scenario2] iter={iters} nodes={final_count} rss_growth=unknown pass");
        return;
    }

    let growth_pct = rss_growth_pct(baseline_rss, last_rss);
    let ratio = (last_rss as f64) / (baseline_rss as f64);
    println!(
        "[soak/scenario2] iter={iters} nodes={final_count} rss_baseline_kb={baseline_rss} \
         rss_final_kb={last_rss} rss_growth={growth_pct:.1}% ratio={ratio:.2}x \
         budget={LEAK_BUDGET}x {}",
        if ratio < LEAK_BUDGET { "pass" } else { "FAIL" }
    );

    assert!(
        ratio < LEAK_BUDGET,
        "RSS grew {ratio:.2}x (>= {LEAK_BUDGET}x leak budget) during churn: \
         baseline={baseline_rss} kB, final={last_rss} kB"
    );
}
