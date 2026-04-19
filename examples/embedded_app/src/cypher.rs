//! openCypher queries through [`sedan_cypher_embedded`].

use sedan_cypher_embedded::{CypherEngine, GraphStore, QueryResult};

use crate::seed::GRAPH;

/// Run an arbitrary Cypher query against the given store.
pub fn run(store: &dyn GraphStore, query: &str) -> QueryResult {
    let engine = CypherEngine::new();
    engine
        .execute(store, GRAPH, query)
        .unwrap_or_else(|e| panic!("cypher query failed: {e}\nquery: {query}"))
}

/// Cypher: friends of Alice, ordered by name.
pub const Q_ALICE_FRIENDS: &str =
    "MATCH (p:Person {name: 'Alice'})-[:FRIEND]->(f:Person) RETURN f.name AS name ORDER BY name";

/// Cypher: people aged > 30, ordered by name.
pub const Q_OVER_30: &str = "MATCH (p:Person) WHERE p.age > 30 RETURN p.name AS name ORDER BY name";
