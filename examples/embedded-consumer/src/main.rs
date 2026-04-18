//! End-to-end example: build a tiny social-graph CLI that uses **all three**
//! SedanDelivery embedded engines from the same Rust process, sharing a single
//! [`InMemoryGraphStore`].
//!
//! Run with:
//!
//! ```bash
//! cd examples/embedded-consumer
//! cargo run
//! ```
//!
//! This is intended as the "happy-path / hello world" for downstream
//! consumers. Everything you see below works against the real embedded crates
//! — no mocks, no test scaffolding.

use indexmap::IndexMap;
use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, Value};
use sedan_gql_embedded::SedanGql;
use sedan_gremlin_embedded::GremlinEngine;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Single shared store. All three engines see the same data.
    let store = InMemoryGraphStore::default();
    store.create_graph("social")?;

    // 2. Seed data through the storage trait directly. This is the lowest-level
    //    way to add data — no parser involved.
    let alice = store.create_node(
        "social",
        vec!["Person".into()],
        IndexMap::from([
            ("name".into(), Value::Str("Alice".into())),
            ("age".into(), Value::Int(30)),
        ]),
    )?;
    let bob = store.create_node(
        "social",
        vec!["Person".into()],
        IndexMap::from([
            ("name".into(), Value::Str("Bob".into())),
            ("age".into(), Value::Int(25)),
        ]),
    )?;
    store.create_edge(
        "social",
        alice,
        bob,
        "KNOWS".into(),
        IndexMap::from([("since".into(), Value::Int(2020))]),
    )?;

    // 3. openCypher
    let cypher = CypherEngine::new();
    let cypher_result = cypher.execute(
        &store,
        "social",
        "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name AS from, b.name AS to",
    )?;
    println!("cypher: {} row(s)", cypher_result.rows.len());
    for row in &cypher_result.rows {
        println!("  {:?} -> {:?}", row["from"], row["to"]);
    }

    // 4. GQL (ISO/IEC 39075)
    let gql = SedanGql::new();
    let gql_result = gql.execute(
        &store,
        "social",
        "MATCH (p:Person) RETURN p.name AS name, p.age AS age ORDER BY age DESC",
    )?;
    println!("gql: {} row(s)", gql_result.rows.len());
    for row in &gql_result.rows {
        println!("  {:?} (age {:?})", row["name"], row["age"]);
    }

    // 5. Gremlin
    let gremlin = GremlinEngine::new();
    let gremlin_result = gremlin.execute(&store, "social", "g.V().count()")?;
    println!(
        "gremlin: g.V().count() = {:?}",
        gremlin_result.rows[0].values().next()
    );

    // 6. Mutate via one engine, read with another. The store is the single
    //    source of truth.
    cypher.execute(
        &store,
        "social",
        "CREATE (:Person {name: 'Carol', age: 40})",
    )?;
    let after = gremlin.execute(&store, "social", "g.V().hasLabel('Person').count()")?;
    println!(
        "after cypher CREATE, gremlin sees {:?} Person vertices",
        after.rows[0].values().next()
    );

    Ok(())
}
