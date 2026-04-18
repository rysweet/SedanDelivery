//! Concurrency tests for sedan-gremlin-embedded.
//!
//! Verifies that GremlinEngine + InMemoryGraphStore can be safely shared
//! across threads (Send + Sync) and that concurrent reads/writes do not
//! corrupt data or panic. Mirrors the structure of the equivalent
//! sedan-cypher-embedded and sedan-gql-embedded concurrency suites so
//! that thread-safety guarantees are uniform across all three embedded
//! engines.

use std::sync::Arc;
use std::thread;

use indexmap::IndexMap;
use sedan_gremlin_embedded::{GraphStore, GremlinEngine, InMemoryGraphStore, Value};

fn fresh_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").expect("create graph");
    store
}

fn seed_persons(store: &InMemoryGraphStore, n: usize) {
    for i in 0..n {
        let mut props = IndexMap::new();
        props.insert("name".into(), Value::Str(format!("p_{i}").into()));
        props.insert("age".into(), Value::Int((i % 100) as i64));
        store
            .create_node("g", vec!["Person".into()], props)
            .expect("create node");
    }
}

// ── Send + Sync compile-time checks ─────────────────────────────────────────

#[test]
fn engine_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<GremlinEngine>();
}

#[test]
fn in_memory_store_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<InMemoryGraphStore>();
}

// ── Concurrent reads ────────────────────────────────────────────────────────

#[test]
fn concurrent_reads_do_not_panic() {
    let engine = Arc::new(GremlinEngine::new());
    let store = Arc::new(fresh_store());
    seed_persons(&store, 50);

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let engine = Arc::clone(&engine);
            let store = Arc::clone(&store);
            thread::spawn(move || {
                for _ in 0..50 {
                    let result = engine
                        .execute(&*store, "g", "g.V().hasLabel('Person').count()")
                        .expect("read should succeed");
                    assert_eq!(result.rows.len(), 1);
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread panicked");
    }
}

#[test]
fn concurrent_reads_return_consistent_count() {
    let engine = Arc::new(GremlinEngine::new());
    let store = Arc::new(fresh_store());
    seed_persons(&store, 100);

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let engine = Arc::clone(&engine);
            let store = Arc::clone(&store);
            thread::spawn(move || {
                for _ in 0..30 {
                    let result = engine
                        .execute(&*store, "g", "g.V().hasLabel('Person').count()")
                        .expect("read should succeed");
                    // Count should be exactly 100 — no torn reads.
                    let count_row = &result.rows[0];
                    let count_val = count_row.values().next().expect("at least one column");
                    match count_val {
                        Value::Int(n) => assert_eq!(*n, 100, "expected exactly 100 vertices"),
                        other => panic!("expected Int count, got {other:?}"),
                    }
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread panicked");
    }
}

// ── Concurrent writes ───────────────────────────────────────────────────────

#[test]
fn concurrent_writes_do_not_corrupt() {
    let engine = Arc::new(GremlinEngine::new());
    let store = Arc::new(fresh_store());

    const THREADS: usize = 8;
    const PER_THREAD: usize = 50;

    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let engine = Arc::clone(&engine);
            let store = Arc::clone(&store);
            thread::spawn(move || {
                for i in 0..PER_THREAD {
                    let q = format!("g.addV('Person').property('id', {})", t * PER_THREAD + i);
                    engine
                        .execute(&*store, "g", &q)
                        .expect("write should succeed");
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread panicked");
    }

    // Verify final count.
    let result = engine
        .execute(&*store, "g", "g.V().hasLabel('Person').count()")
        .expect("count should succeed");
    let count_val = result.rows[0].values().next().expect("at least one column");
    match count_val {
        Value::Int(n) => assert_eq!(*n, (THREADS * PER_THREAD) as i64),
        other => panic!("expected Int count, got {other:?}"),
    }
}

// ── Concurrent reads and writes ─────────────────────────────────────────────

#[test]
fn concurrent_reads_and_writes_do_not_panic() {
    let engine = Arc::new(GremlinEngine::new());
    let store = Arc::new(fresh_store());
    seed_persons(&store, 25);

    let mut handles = Vec::new();

    // 4 reader threads
    for _ in 0..4 {
        let engine = Arc::clone(&engine);
        let store = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            for _ in 0..40 {
                let _ = engine
                    .execute(&*store, "g", "g.V().hasLabel('Person').count()")
                    .expect("read should succeed");
            }
        }));
    }

    // 4 writer threads
    for t in 0..4 {
        let engine = Arc::clone(&engine);
        let store = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            for i in 0..20 {
                let q = format!("g.addV('Worker').property('tid', {})", t * 100 + i);
                engine
                    .execute(&*store, "g", &q)
                    .expect("write should succeed");
            }
        }));
    }

    for h in handles {
        h.join().expect("thread panicked");
    }

    // Final invariants: 25 Person + 80 Worker = 105 vertices.
    let result = engine
        .execute(&*store, "g", "g.V().count()")
        .expect("count should succeed");
    let total = result.rows[0].values().next().unwrap();
    assert_eq!(total, &Value::Int(105));
}

// ── Multiple stores in parallel ─────────────────────────────────────────────

#[test]
fn multiple_independent_stores_work_in_parallel() {
    let engine = Arc::new(GremlinEngine::new());

    let handles: Vec<_> = (0..6)
        .map(|tid| {
            let engine = Arc::clone(&engine);
            thread::spawn(move || {
                let store = fresh_store();
                for i in 0..30 {
                    let q = format!("g.addV('N').property('i', {})", tid * 100 + i);
                    engine
                        .execute(&store, "g", &q)
                        .expect("write should succeed");
                }
                let result = engine
                    .execute(&store, "g", "g.V().count()")
                    .expect("count should succeed");
                let n = result.rows[0].values().next().unwrap();
                assert_eq!(n, &Value::Int(30));
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread panicked");
    }
}
