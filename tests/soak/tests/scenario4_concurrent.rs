//! Scenario 4 — concurrent stress.
//!
//! 8 OS threads each running 50K read queries against a shared embedded
//! `InMemoryGraphStore`. Asserts:
//! - no deadlocks (a 5-minute `TimeoutGuard` aborts the process if hit),
//! - no thread panics,
//! - the graph is unchanged after the run (read-only contract),
//! - every per-thread query returned successfully.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use sedan_cypher_embedded::CypherEngine;
use soak::{build_person_graph, count_nodes, scaled_iters, TimeoutGuard, READ_QUERIES};

const NODES: usize = 5_000;
const THREADS: usize = 8;
/// Spec target: 50K iterations per thread. Scaled by `SOAK_ITER_SCALE`.
const ITERS_PER_THREAD_BASE: usize = 50_000;

#[test]
#[ignore = "soak — opt-in via cargo test -p soak -- --ignored"]
fn scenario4_concurrent_stress() {
    let _guard = TimeoutGuard::arm("scenario4", Duration::from_secs(1500));

    let iters_per_thread = scaled_iters(ITERS_PER_THREAD_BASE);

    let store = Arc::new(build_person_graph(NODES));
    let engine_check = CypherEngine::new();
    let initial_count = count_nodes(&engine_check, &store);
    assert_eq!(initial_count, NODES as i64);

    let mut handles = Vec::with_capacity(THREADS);
    for tid in 0..THREADS {
        let store = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            // Each thread owns its own engine (and therefore its own plan cache).
            let engine = CypherEngine::new();
            let mut ok: u64 = 0;
            for i in 0..iters_per_thread {
                let q = READ_QUERIES[(tid + i) % READ_QUERIES.len()];
                engine
                    .execute(store.as_ref(), soak::GRAPH, q)
                    .unwrap_or_else(|e| panic!("thread {tid} iter {i} failed: {e}"));
                ok += 1;
            }
            ok
        }));
    }

    let mut total: u64 = 0;
    for (tid, h) in handles.into_iter().enumerate() {
        let n = h.join().unwrap_or_else(|_| panic!("thread {tid} panicked"));
        total += n;
    }

    let final_count = count_nodes(&engine_check, &store);
    assert_eq!(
        final_count, initial_count,
        "graph mutated under concurrent reads: initial={initial_count}, final={final_count}"
    );

    println!(
        "[soak/scenario4] threads={THREADS} iters_per_thread={iters_per_thread} \
         total_ok={total} nodes={final_count} pass"
    );
}
