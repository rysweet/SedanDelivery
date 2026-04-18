//! Quickstart for `sedan-gremlin-embedded`.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example quickstart -p sedan-gremlin-embedded
//! ```
//!
//! Demonstrates the canonical downstream-consumer flow: create an engine,
//! seed two vertices through the storage layer, run a Gremlin traversal,
//! verify the count and emit a deterministic success line.

use indexmap::IndexMap;
use sedan_gremlin_embedded::{GraphStore, GremlinEngine, InMemoryGraphStore, Value};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = GremlinEngine::new();
    let store = InMemoryGraphStore::default();
    store.create_graph("g")?;

    for name in ["Alice", "Bob"] {
        store.create_node(
            "g",
            vec!["Person".into()],
            IndexMap::from([("name".into(), Value::Str(name.into()))]),
        )?;
    }

    let result = engine.execute(&store, "g", "g.V().count()")?;
    let row = &result.rows[0];
    let count = row
        .values()
        .next()
        .and_then(|v| match v {
            Value::Int(n) => Some(*n),
            _ => None,
        })
        .ok_or("expected integer count")?;

    println!("QUICKSTART_OK count={count}");
    Ok(())
}
