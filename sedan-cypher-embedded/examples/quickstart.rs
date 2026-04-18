//! Quickstart for `sedan-cypher-embedded`.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example quickstart -p sedan-cypher-embedded
//! ```
//!
//! Demonstrates the canonical downstream-consumer flow: create an engine,
//! create a graph, run a write query, run a read query, print results.

use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, Value};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = CypherEngine::new();
    let store = InMemoryGraphStore::default();
    store.create_graph("g")?;

    engine.execute(
        &store,
        "g",
        "CREATE (:Person {name: 'Alice', age: 30}), \
                 (:Person {name: 'Bob',   age: 25})",
    )?;

    let result = engine.execute(
        &store,
        "g",
        "MATCH (p:Person) RETURN count(p) AS n, avg(p.age) AS avg_age",
    )?;

    let row = &result.rows[0];
    let n = match row.get("n") {
        Some(Value::Int(v)) => *v,
        other => return Err(format!("expected Int for n, got {other:?}").into()),
    };
    let avg = match row.get("avg_age") {
        Some(Value::Float(v)) => *v,
        Some(Value::Int(v)) => *v as f64,
        other => return Err(format!("expected numeric for avg_age, got {other:?}").into()),
    };

    println!("QUICKSTART_OK count={n} avg_age={avg}");
    Ok(())
}
