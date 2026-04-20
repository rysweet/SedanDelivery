//! Concurrent stress scenarios for `sedan-cypher-embedded`.

use std::sync::Arc;
use std::time::Duration;

use concurrent_stress::{
    person_count, print_report, run_burst, run_scenario, seeded_store, BURST_THREADS, GRAPH,
    SCENARIO_DURATION, SOAK_DURATION,
};
use sedan_cypher_embedded::CypherEngine;

const SEED_NODES: usize = 200;

fn engine() -> Arc<CypherEngine> {
    Arc::new(CypherEngine::new())
}

fn read_op(
    engine: Arc<CypherEngine>,
) -> impl Fn(&cypher_core::storage::InMemoryGraphStore, u64) -> Result<(), ()> + Send + Sync + 'static
{
    move |store, _seed| {
        // A simple read that always returns *some* row in a non-empty graph.
        match engine.execute(store, GRAPH, "MATCH (p:Person) RETURN count(p) AS n") {
            Ok(r) => {
                // Validity: must have exactly one row with an integer count >= 0.
                if r.rows.len() != 1 {
                    return Err(());
                }
                match r.rows[0].get("n") {
                    Some(cypher_core::types::Value::Int(n)) if *n >= 0 => Ok(()),
                    _ => Err(()),
                }
            }
            Err(_) => Err(()),
        }
    }
}

fn write_op(
    engine: Arc<CypherEngine>,
) -> impl Fn(&cypher_core::storage::InMemoryGraphStore, u64, u64) -> Result<(), ()> + Send + Sync + 'static
{
    move |store, seed, counter| {
        // Deterministic-per-thread node id so failures reproduce.
        let id = seed.wrapping_add(counter);
        let q = format!("CREATE (:Person {{name: 'w_{id}', age: {}}})", id % 100);
        engine.execute(store, GRAPH, &q).map(|_| ()).map_err(|_| ())
    }
}

fn run_default(duration: Duration) {
    // 1) Read-heavy.
    let store = seeded_store(SEED_NODES);
    let eng = engine();
    let report = run_scenario(
        "cypher/read-heavy",
        store,
        8,
        1,
        duration,
        read_op(Arc::clone(&eng)),
        write_op(Arc::clone(&eng)),
    );
    print_report(&report);
    assert_eq!(report.read_errors, 0, "read-heavy: readers must not error");

    // 2) Write-heavy — assert final node count matches expected.
    let store = seeded_store(SEED_NODES);
    let eng = engine();
    let report = run_scenario(
        "cypher/write-heavy",
        Arc::clone(&store),
        1,
        8,
        duration,
        read_op(Arc::clone(&eng)),
        write_op(Arc::clone(&eng)),
    );
    print_report(&report);
    let expected = SEED_NODES as u64 + (report.writes - report.write_errors);
    let actual = person_count(&store);
    assert_eq!(actual, expected,
        "cypher/write-heavy: post-run :Person count {actual} != expected {expected} (writes={}, errors={})",
        report.writes, report.write_errors);

    // 3) Mixed.
    let store = seeded_store(SEED_NODES);
    let eng = engine();
    let report = run_scenario(
        "cypher/mixed",
        store,
        4,
        4,
        duration,
        read_op(Arc::clone(&eng)),
        write_op(Arc::clone(&eng)),
    );
    print_report(&report);

    // 4) Burst — 32 threads spawn and each runs one query.
    let store = seeded_store(SEED_NODES);
    let eng = engine();
    run_burst("cypher/burst", store, BURST_THREADS, move |store, tid| {
        let q = if tid % 2 == 0 {
            "MATCH (p:Person) RETURN count(p) AS n".to_string()
        } else {
            format!("CREATE (:Person {{name: 'b_{tid}', age: {}}})", tid % 100)
        };
        eng.execute(store, GRAPH, &q).expect("burst query failed");
    });
    println!("[stress] cypher/burst             completed {BURST_THREADS} threads");
}

#[test]
fn cypher_concurrent_stress() {
    run_default(SCENARIO_DURATION);
}

#[test]
#[ignore = "soak — run with `cargo test -p concurrent-stress -- --ignored`"]
fn cypher_concurrent_soak() {
    run_default(SOAK_DURATION);
}
