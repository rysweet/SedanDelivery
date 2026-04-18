//! Concurrency tests for sedan-gql-embedded.
//!
//! Verifies that SedanGql can be used safely from multiple threads
//! sharing the same store, and that multiple SedanGql instances work independently.

use std::sync::Arc;
use std::thread;

use sedan_gql_embedded::{GraphStore, InMemoryGraphStore, SedanGql};

fn shared_store() -> Arc<InMemoryGraphStore> {
    let store = Arc::new(InMemoryGraphStore::default());
    store.create_graph("g").unwrap();
    store
}

#[test]
fn concurrent_reads_through_public_api() {
    let store = shared_store();
    let engine = SedanGql::new();

    // Seed data
    engine
        .execute(&*store, "g", "CREATE (n:Person {name: 'Alice'})")
        .unwrap();
    engine
        .execute(&*store, "g", "CREATE (n:Person {name: 'Bob'})")
        .unwrap();

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let s = Arc::clone(&store);
            thread::spawn(move || {
                let e = SedanGql::new();
                for _ in 0..50 {
                    let result = e.execute(&*s, "g", "MATCH (n:Person) RETURN n.name");
                    assert!(result.is_ok());
                    assert!(result.unwrap().rows.len() >= 2);
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread panicked");
    }
}

#[test]
fn concurrent_writes_through_public_api() {
    let store = shared_store();

    let handles: Vec<_> = (0..8)
        .map(|i| {
            let s = Arc::clone(&store);
            thread::spawn(move || {
                let e = SedanGql::new();
                for j in 0..20 {
                    let query = format!(
                        "INSERT (n:Worker {{id: {}, batch: {}}}) RETURN n.id",
                        i * 100 + j,
                        i
                    );
                    let result = e.execute(&*s, "g", &query);
                    assert!(result.is_ok(), "write failed: {:?}", result.err());
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread panicked");
    }

    let stats = store.graph_stats("g").unwrap();
    assert_eq!(stats.node_count, 160);
}

#[test]
fn concurrent_mixed_gql_operations() {
    let store = shared_store();

    // Pre-seed some data for FILTER queries
    let engine = SedanGql::new();
    engine
        .execute(&*store, "g", "INSERT (n:Seed {val: 100})")
        .unwrap();

    let handles: Vec<_> = (0..6)
        .map(|i| {
            let s = Arc::clone(&store);
            thread::spawn(move || {
                let e = SedanGql::new();
                for j in 0..30 {
                    match j % 3 {
                        0 => {
                            // GQL INSERT
                            let q = format!("INSERT (n:Item {{val: {}}})", i * 100 + j);
                            let _ = e.execute(&*s, "g", &q);
                        }
                        1 => {
                            // GQL FILTER
                            let result = e.execute(
                                &*s,
                                "g",
                                "MATCH (n:Seed) FILTER n.val = 100 RETURN n.val",
                            );
                            assert!(result.is_ok());
                        }
                        _ => {
                            // GQL FOR
                            let result = e.execute(&*s, "g", "FOR x IN [1, 2, 3] RETURN x");
                            assert!(result.is_ok());
                            assert_eq!(result.unwrap().rows.len(), 3);
                        }
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
fn concurrent_mandatory_match() {
    let store = shared_store();
    let engine = SedanGql::new();
    engine
        .execute(&*store, "g", "CREATE (n:Anchor {val: 1})")
        .unwrap();

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let s = Arc::clone(&store);
            thread::spawn(move || {
                let e = SedanGql::new();
                for _ in 0..30 {
                    // Should succeed
                    let ok = e.execute(&*s, "g", "MANDATORY MATCH (n:Anchor) RETURN n.val");
                    assert!(ok.is_ok());

                    // Should fail
                    let err = e.execute(&*s, "g", "MANDATORY MATCH (n:Ghost) RETURN n");
                    assert!(err.is_err());
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread panicked");
    }
}

#[test]
fn concurrent_graph_isolation() {
    // Multiple threads writing to different graphs in the same store
    let store = Arc::new(InMemoryGraphStore::default());
    store.create_graph("a").unwrap();
    store.create_graph("b").unwrap();

    let handles: Vec<_> = ["a", "b"]
        .iter()
        .map(|&graph| {
            let s = Arc::clone(&store);
            let g = graph.to_string();
            thread::spawn(move || {
                let e = SedanGql::new();
                for i in 0..30 {
                    let q = format!("INSERT (n:Node {{val: {}}})", i);
                    e.execute(&*s, &g, &q).unwrap();
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread panicked");
    }

    let a_stats = store.graph_stats("a").unwrap();
    let b_stats = store.graph_stats("b").unwrap();
    assert_eq!(a_stats.node_count, 30);
    assert_eq!(b_stats.node_count, 30);
}
