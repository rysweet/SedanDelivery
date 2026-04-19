//! `embedded_quickstart` — the canonical "hello world" for SedanDelivery's
//! three embeddable graph engines.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p embedded_quickstart
//! ```
//!
//! The program:
//!
//! 1. Builds an in-memory `GraphStore` (no Redis, no network).
//! 2. Inserts three `Person` nodes via each engine in turn.
//! 3. Runs a read query through that engine and prints the result.
//!
//! The three engines are completely independent at the API level but share
//! the same `cypher-core` storage layer, so a single `InMemoryGraphStore`
//! could host all of them at once. We use a fresh store per engine here so
//! each section is self-contained and easy to copy-paste.

use indexmap::IndexMap;
use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, Value};
use sedan_gql_embedded::SedanGql;
use sedan_gremlin_embedded::GremlinEngine;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("== sedan-cypher-embedded ==");
    cypher_demo()?;

    println!("\n== sedan-gql-embedded ==");
    gql_demo()?;

    println!("\n== sedan-gremlin-embedded ==");
    gremlin_demo()?;

    println!("\nQUICKSTART_OK");
    Ok(())
}

fn cypher_demo() -> Result<(), Box<dyn std::error::Error>> {
    let engine = CypherEngine::new();
    let store = InMemoryGraphStore::default();
    store.create_graph("g")?;

    engine.execute(
        &store,
        "g",
        "CREATE (:Person {name: 'Alice', age: 30}), \
                 (:Person {name: 'Bob',   age: 25}), \
                 (:Person {name: 'Carol', age: 41})",
    )?;

    let result = engine.execute(
        &store,
        "g",
        "MATCH (p:Person) RETURN p.name AS name, p.age AS age ORDER BY p.age",
    )?;

    for row in &result.rows {
        println!(
            "  {} ({})",
            value_str(row.get("name")),
            value_str(row.get("age"))
        );
    }
    Ok(())
}

fn gql_demo() -> Result<(), Box<dyn std::error::Error>> {
    let engine = SedanGql::new();
    let store = InMemoryGraphStore::default();
    store.create_graph("g")?;

    engine.execute(
        &store,
        "g",
        "INSERT (:Person {name: 'Alice', age: 30}), \
                 (:Person {name: 'Bob',   age: 25}), \
                 (:Person {name: 'Carol', age: 41})",
    )?;

    let result = engine.execute(
        &store,
        "g",
        "MATCH (p:Person) RETURN p.name AS name, p.age AS age ORDER BY p.age",
    )?;

    for row in &result.rows {
        println!(
            "  {} ({})",
            value_str(row.get("name")),
            value_str(row.get("age"))
        );
    }
    Ok(())
}

fn gremlin_demo() -> Result<(), Box<dyn std::error::Error>> {
    let engine = GremlinEngine::new();
    let store = InMemoryGraphStore::default();
    store.create_graph("g")?;

    for (name, age) in [("Alice", 30_i64), ("Bob", 25), ("Carol", 41)] {
        store.create_node(
            "g",
            vec!["Person".into()],
            IndexMap::from([
                ("name".into(), Value::Str(name.into())),
                ("age".into(), Value::Int(age)),
            ]),
        )?;
    }

    let result = engine.execute(
        &store,
        "g",
        r#"g.V().hasLabel("Person").order().by("age").values("name")"#,
    )?;

    for row in &result.rows {
        // Gremlin's values(...) step returns a single column per row; pick the first.
        if let Some((_, v)) = row.iter().next() {
            println!("  {}", value_str(Some(v)));
        }
    }
    Ok(())
}

fn value_str(v: Option<&Value>) -> String {
    match v {
        Some(Value::Str(s)) => s.to_string(),
        Some(Value::Int(i)) => i.to_string(),
        Some(Value::Float(f)) => f.to_string(),
        Some(other) => format!("{other:?}"),
        None => "<missing>".to_string(),
    }
}
