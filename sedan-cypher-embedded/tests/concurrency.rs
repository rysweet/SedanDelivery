//! Concurrency tests for sedan-cypher-embedded.
//!
//! Verifies that CypherEngine + InMemoryGraphStore can be safely shared
//! across threads (Send + Sync) and that concurrent reads/writes do not
//! corrupt data or panic.

use std::sync::Arc;
use std::thread;

use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, Value};

fn fresh_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").expect("create graph");
    store
}

// ── Send + Sync compile-time checks ─────────────────────────────────────────

#[test]
fn engine_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CypherEngine>();
}

#[test]
fn in_memory_store_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<InMemoryGraphStore>();
}

// ── Concurrent reads ────────────────────────────────────────────────────────

#[test]
fn concurrent_reads_do_not_panic() {
    let engine = Arc::new(CypherEngine::new());
    let store = Arc::new(fresh_store());

    // Seed some data
    engine
        .execute(&*store, "g", "CREATE (:Person {name: 'Alice'})")
        .unwrap();

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let engine = Arc::clone(&engine);
            let store = Arc::clone(&store);
            thread::spawn(move || {
                for _ in 0..50 {
                    let result = engine
                        .execute(&*store, "g", "MATCH (p:Person) RETURN p.name AS name")
                        .unwrap();
                    assert_eq!(result.rows.len(), 1);
                    assert_eq!(result.rows[0]["name"], Value::Str("Alice".into()));
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("reader thread panicked");
    }
}

// ── Concurrent writes ───────────────────────────────────────────────────────

#[test]
fn concurrent_writes_do_not_lose_data() {
    let engine = Arc::new(CypherEngine::new());
    let store = Arc::new(fresh_store());

    let num_threads = 4;
    let creates_per_thread = 25;

    let handles: Vec<_> = (0..num_threads)
        .map(|t| {
            let engine = Arc::clone(&engine);
            let store = Arc::clone(&store);
            thread::spawn(move || {
                for i in 0..creates_per_thread {
                    let name = format!("t{t}_n{i}");
                    engine
                        .execute(&*store, "g", &format!("CREATE (:W {{name: '{name}'}})"))
                        .unwrap();
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("writer thread panicked");
    }

    let expected = num_threads * creates_per_thread;
    let result = engine
        .execute(&*store, "g", "MATCH (w:W) RETURN count(w) AS c")
        .unwrap();
    assert_eq!(
        result.rows[0]["c"],
        Value::Int(expected as i64),
        "all {expected} nodes must be present after concurrent writes"
    );
}

// ── Mixed reads and writes ──────────────────────────────────────────────────

#[test]
fn mixed_concurrent_reads_and_writes_no_panic() {
    let engine = Arc::new(CypherEngine::new());
    let store = Arc::new(fresh_store());

    // Seed a node so readers always have something
    engine
        .execute(&*store, "g", "CREATE (:Seed {v: 0})")
        .unwrap();

    let writers: Vec<_> = (0..4)
        .map(|t| {
            let engine = Arc::clone(&engine);
            let store = Arc::clone(&store);
            thread::spawn(move || {
                for i in 0..20 {
                    let _ = engine.execute(
                        &*store,
                        "g",
                        &format!("CREATE (:Mixed {{t: {t}, i: {i}}})"),
                    );
                }
            })
        })
        .collect();

    let readers: Vec<_> = (0..4)
        .map(|_| {
            let engine = Arc::clone(&engine);
            let store = Arc::clone(&store);
            thread::spawn(move || {
                for _ in 0..30 {
                    // This may see varying counts; the important thing is no panic.
                    let _ = engine.execute(&*store, "g", "MATCH (n) RETURN count(n) AS c");
                }
            })
        })
        .collect();

    for h in writers.into_iter().chain(readers) {
        h.join().expect("thread panicked during mixed read/write");
    }
}

// ── Graph isolation under concurrency ───────────────────────────────────────

#[test]
fn concurrent_operations_on_different_graphs_are_isolated() {
    let engine = Arc::new(CypherEngine::new());
    let store = Arc::new(InMemoryGraphStore::default());
    store.create_graph("x").unwrap();
    store.create_graph("y").unwrap();

    let h1 = {
        let engine = Arc::clone(&engine);
        let store = Arc::clone(&store);
        thread::spawn(move || {
            for i in 0..30 {
                engine
                    .execute(&*store, "x", &format!("CREATE (:XNode {{i: {i}}})"))
                    .unwrap();
            }
        })
    };

    let h2 = {
        let engine = Arc::clone(&engine);
        let store = Arc::clone(&store);
        thread::spawn(move || {
            for i in 0..30 {
                engine
                    .execute(&*store, "y", &format!("CREATE (:YNode {{i: {i}}})"))
                    .unwrap();
            }
        })
    };

    h1.join().unwrap();
    h2.join().unwrap();

    let x_count = engine
        .execute(&*store, "x", "MATCH (n:XNode) RETURN count(n) AS c")
        .unwrap();
    let y_count = engine
        .execute(&*store, "y", "MATCH (n:YNode) RETURN count(n) AS c")
        .unwrap();

    assert_eq!(x_count.rows[0]["c"], Value::Int(30));
    assert_eq!(y_count.rows[0]["c"], Value::Int(30));

    // Cross-graph: YNode should not appear in graph x
    let cross = engine
        .execute(&*store, "x", "MATCH (n:YNode) RETURN count(n) AS c")
        .unwrap();
    assert_eq!(cross.rows[0]["c"], Value::Int(0));
}
