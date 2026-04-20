//! Concurrent stress scenarios for `sedan-gremlin-embedded`.

use std::sync::Arc;
use std::time::Duration;

use concurrent_stress::{
    person_count, print_report, run_burst, run_scenario, seeded_store, BURST_THREADS, GRAPH,
    SCENARIO_DURATION, SOAK_DURATION,
};
use sedan_gremlin_embedded::GremlinEngine;

const SEED_NODES: usize = 200;

fn engine() -> Arc<GremlinEngine> {
    Arc::new(GremlinEngine::new())
}

fn read_op(
    engine: Arc<GremlinEngine>,
) -> impl Fn(&cypher_core::storage::InMemoryGraphStore, u64) -> Result<(), ()> + Send + Sync + 'static
{
    move |store, _seed| {
        match engine.execute(store, GRAPH, "g.V().hasLabel('Person').count()") {
            Ok(r) => {
                if r.rows.is_empty() {
                    return Err(());
                }
                // Gremlin count() returns a single Int/Long-shaped value.
                let row = &r.rows[0];
                if row.is_empty() {
                    return Err(());
                }
                let v = row.iter().next().map(|(_, v)| v);
                match v {
                    Some(cypher_core::types::Value::Int(n)) if *n >= 0 => Ok(()),
                    _ => Err(()),
                }
            }
            Err(_) => Err(()),
        }
    }
}

fn write_op(
    engine: Arc<GremlinEngine>,
) -> impl Fn(&cypher_core::storage::InMemoryGraphStore, u64, u64) -> Result<(), ()> + Send + Sync + 'static
{
    move |store, seed, counter| {
        let id = seed.wrapping_add(counter);
        let q = format!(
            "g.addV('Person').property('name','w_{id}').property('age',{})",
            id % 100
        );
        engine.execute(store, GRAPH, &q).map(|_| ()).map_err(|_| ())
    }
}

fn run_default(duration: Duration) {
    let store = seeded_store(SEED_NODES);
    let eng = engine();
    let report = run_scenario(
        "gremlin/read-heavy",
        store,
        8,
        1,
        duration,
        read_op(Arc::clone(&eng)),
        write_op(Arc::clone(&eng)),
    );
    print_report(&report);
    assert_eq!(
        report.read_errors, 0,
        "gremlin/read-heavy: readers must not error"
    );

    let store = seeded_store(SEED_NODES);
    let eng = engine();
    let report = run_scenario(
        "gremlin/write-heavy",
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
        "gremlin/write-heavy: post-run :Person count {actual} != expected {expected} (writes={}, errors={})",
        report.writes, report.write_errors);

    let store = seeded_store(SEED_NODES);
    let eng = engine();
    let report = run_scenario(
        "gremlin/mixed",
        store,
        4,
        4,
        duration,
        read_op(Arc::clone(&eng)),
        write_op(Arc::clone(&eng)),
    );
    print_report(&report);

    let store = seeded_store(SEED_NODES);
    let eng = engine();
    run_burst("gremlin/burst", store, BURST_THREADS, move |store, tid| {
        let q = if tid % 2 == 0 {
            "g.V().hasLabel('Person').count()".to_string()
        } else {
            format!(
                "g.addV('Person').property('name','b_{tid}').property('age',{})",
                tid % 100
            )
        };
        eng.execute(store, GRAPH, &q).expect("burst query failed");
    });
    println!("[stress] gremlin/burst            completed {BURST_THREADS} threads");
}

#[test]
fn gremlin_concurrent_stress() {
    run_default(SCENARIO_DURATION);
}

#[test]
#[ignore = "soak — run with `cargo test -p concurrent-stress -- --ignored`"]
fn gremlin_concurrent_soak() {
    run_default(SOAK_DURATION);
}
