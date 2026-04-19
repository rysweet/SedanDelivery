//! Sophisticated `sedan-gremlin-embedded` example.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example sophisticated -p sedan-gremlin-embedded
//! ```
//!
//! Seeds a small social graph through the storage API (Person nodes +
//! KNOWS edges), then runs several Gremlin traversals: a count, a
//! property filter, and a 2-hop projection. Demonstrates the
//! `compile` / `execute_compiled` pattern for re-using a query, and
//! enforcing an [`ExecutionBudget`].

use std::time::Duration;

use indexmap::IndexMap;
use sedan_gremlin_embedded::prelude::*;

fn person(name: &str, age: i64) -> IndexMap<String, Value> {
    IndexMap::from([
        ("name".into(), Value::Str(name.into())),
        ("age".into(), Value::Int(age)),
    ])
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = GremlinEngine::new();
    let store = InMemoryGraphStore::default();
    store.create_graph("social")?;

    // Seed four Person vertices through the storage API.
    let mut ids = Vec::new();
    for (name, age) in [("Alice", 30), ("Bob", 25), ("Carol", 35), ("Dave", 40)] {
        ids.push(store.create_node("social", vec!["Person".into()], person(name, age))?);
    }

    // KNOWS edges: Alice→Bob, Bob→Carol, Carol→Dave, Alice→Carol.
    for (from, to) in [(0, 1), (1, 2), (2, 3), (0, 2)] {
        store.create_edge(
            "social",
            ids[from],
            ids[to],
            "KNOWS".into(),
            IndexMap::new(),
        )?;
    }

    // 1) Count vertices via the one-shot execute path.
    let count = engine.execute(&store, "social", "g.V().count()")?;
    println!("V_COUNT row={:?}", count.rows[0]);

    // 2) Compile-once / execute-many: filter by name, project a property.
    let by_name = engine.compile("g.V().has(\"name\",\"Alice\").values(\"name\")")?;
    let alice = engine.execute_compiled(&store, "social", &by_name)?;
    let again = engine.execute_compiled(&store, "social", &by_name)?;
    println!(
        "ALICE_LOOKUPS first_rows={} second_rows={}",
        alice.rows.len(),
        again.rows.len(),
    );

    // 3) Two-hop traversal with an explicit execution budget.
    let mut budget = ExecutionBudget::new(Duration::from_secs(5), 10_000);
    let two_hop = engine.execute_with_budget(
        &store,
        "social",
        "g.V().has(\"name\",\"Alice\").out(\"KNOWS\").out(\"KNOWS\").values(\"name\")",
        &mut budget,
    )?;
    println!("TWO_HOP_FROM_ALICE rows={}", two_hop.rows.len());
    for row in &two_hop.rows {
        for (col, val) in row.iter() {
            println!("  - {col} = {val:?}");
        }
    }

    println!("SOPHISTICATED_OK vertices={} edges=4", ids.len());
    Ok(())
}
