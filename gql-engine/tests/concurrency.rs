//! Concurrency tests for gql-engine.
//!
//! Verifies that the engine is safe to use from multiple threads sharing
//! the same InMemoryGraphStore (which is Send + Sync via interior mutability).

use std::sync::Arc;
use std::thread;

use gql_engine::{execute, GraphStore, InMemoryGraphStore};

fn shared_store() -> Arc<InMemoryGraphStore> {
    let store = Arc::new(InMemoryGraphStore::new());
    store.create_graph("g").unwrap();
    store
}

#[test]
fn concurrent_reads_do_not_panic() {
    let store = shared_store();
    // Seed some data
    execute("CREATE (n:Person {name: 'Alice', age: 30})", &*store, "g").unwrap();
    execute("CREATE (n:Person {name: 'Bob', age: 25})", &*store, "g").unwrap();

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let s = Arc::clone(&store);
            thread::spawn(move || {
                for _ in 0..50 {
                    let result = execute("MATCH (n:Person) RETURN n.name", &*s, "g");
                    assert!(result.is_ok(), "concurrent read failed: {:?}", result.err());
                    let qr = result.unwrap();
                    assert!(qr.rows.len() >= 2);
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread panicked");
    }
}

#[test]
fn concurrent_writes_do_not_panic() {
    let store = shared_store();

    let handles: Vec<_> = (0..8)
        .map(|i| {
            let s = Arc::clone(&store);
            thread::spawn(move || {
                for j in 0..20 {
                    let query = format!(
                        "CREATE (n:Worker {{id: {}, batch: {}}}) RETURN n.id",
                        i * 100 + j,
                        i
                    );
                    let result = execute(&query, &*s, "g");
                    assert!(
                        result.is_ok(),
                        "concurrent write failed: {:?}",
                        result.err()
                    );
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread panicked");
    }

    // Verify all nodes were created
    let stats = store.graph_stats("g").unwrap();
    assert_eq!(stats.node_count, 8 * 20, "expected 160 nodes");
}

#[test]
fn concurrent_mixed_reads_and_writes() {
    let store = shared_store();
    // Seed initial data
    execute("CREATE (n:City {name: 'London'})", &*store, "g").unwrap();

    let handles: Vec<_> = (0..8)
        .map(|i| {
            let s = Arc::clone(&store);
            thread::spawn(move || {
                for j in 0..25 {
                    if j % 2 == 0 {
                        // Write using GQL INSERT
                        let query = format!("INSERT (n:City {{name: 'City_{}_{}'}})", i, j);
                        let _ = execute(&query, &*s, "g");
                    } else {
                        // Read using GQL FILTER
                        let result = execute(
                            "MATCH (n:City) FILTER n.name = 'London' RETURN n.name",
                            &*s,
                            "g",
                        );
                        assert!(result.is_ok(), "concurrent read failed: {:?}", result.err());
                    }
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread panicked");
    }
}

#[test]
fn concurrent_gql_filter_queries() {
    let store = shared_store();
    // Seed data
    for i in 0..20 {
        execute(&format!("CREATE (n:Item {{val: {}}})", i), &*store, "g").unwrap();
    }

    let handles: Vec<_> = (0..4)
        .map(|threshold| {
            let s = Arc::clone(&store);
            thread::spawn(move || {
                for _ in 0..30 {
                    let query = format!(
                        "MATCH (n:Item) FILTER n.val > {} RETURN n.val",
                        threshold * 5
                    );
                    let result = execute(&query, &*s, "g");
                    assert!(result.is_ok());
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread panicked");
    }
}

#[test]
fn concurrent_mandatory_match_does_not_deadlock() {
    let store = shared_store();
    // Some threads will get errors (no data), some will succeed (after data exists)
    execute("CREATE (n:Anchor {val: 1})", &*store, "g").unwrap();

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let s = Arc::clone(&store);
            thread::spawn(move || {
                for _ in 0..20 {
                    // This should succeed since Anchor exists
                    let result = execute("MANDATORY MATCH (n:Anchor) RETURN n.val", &*s, "g");
                    assert!(result.is_ok(), "mandatory match failed: {:?}", result.err());

                    // This should fail since Ghost doesn't exist
                    let result = execute("MANDATORY MATCH (n:Ghost) RETURN n", &*s, "g");
                    assert!(result.is_err());
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread panicked");
    }
}
