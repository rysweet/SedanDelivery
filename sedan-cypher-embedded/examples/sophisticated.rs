//! Sophisticated `sedan-cypher-embedded` example.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example sophisticated -p sedan-cypher-embedded
//! ```
//!
//! Builds a small social graph (Person nodes + KNOWS edges), runs a
//! parameterised friend-of-friend query through `execute_with_parameters`,
//! iterates result rows, and reports mutation statistics.

use std::collections::HashMap;

use sedan_cypher_embedded::{
    CypherEngine, GraphStore, InMemoryGraphStore, ProcedureRegistry, Value,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = CypherEngine::new();
    let store = InMemoryGraphStore::default();
    store.create_graph("social")?;

    // Build a small social graph in one statement.
    let seed = engine.execute(
        &store,
        "social",
        "CREATE (alice:Person {name: 'Alice', age: 30}),
                (bob:Person   {name: 'Bob',   age: 25}),
                (carol:Person {name: 'Carol', age: 35}),
                (dave:Person  {name: 'Dave',  age: 40}),
                (alice)-[:KNOWS]->(bob),
                (bob)-[:KNOWS]->(carol),
                (carol)-[:KNOWS]->(dave),
                (alice)-[:KNOWS]->(carol)",
    )?;
    println!(
        "SEEDED nodes_created={} edges_created={}",
        seed.stats.nodes_created, seed.stats.edges_created,
    );

    // Parameterised friend-of-friend query: find people Alice knows
    // transitively (1 or 2 hops) who are at least `$min_age` years old.
    let mut params = HashMap::new();
    params.insert("start_name".to_string(), Value::Str("Alice".into()));
    params.insert("min_age".to_string(), Value::Int(28));

    let registry = ProcedureRegistry::new();
    let result = engine.execute_with_parameters(
        &store,
        "social",
        "MATCH (a:Person {name: $start_name})-[:KNOWS*1..2]->(p:Person)
         WHERE p.age >= $min_age AND p.name <> $start_name
         RETURN DISTINCT p.name AS name, p.age AS age
         ORDER BY age DESC",
        &params,
        &registry,
    )?;

    println!("COLUMNS: {:?}", result.columns);
    for row in &result.rows {
        let name = match row.get("name") {
            Some(Value::Str(s)) => &**s,
            _ => "?",
        };
        let age = match row.get("age") {
            Some(Value::Int(n)) => *n,
            _ => -1,
        };
        println!("  - {name} ({age})");
    }

    // Cache hit on the un-parameterised path.
    let avg = engine.execute(
        &store,
        "social",
        "MATCH (p:Person) RETURN avg(p.age) AS avg_age",
    )?;
    println!("AVG_AGE row={:?}", avg.rows[0]);

    println!("SOPHISTICATED_OK rows={}", result.rows.len());
    Ok(())
}
