//! GQL (ISO/IEC 39075) queries through [`sedan_gql_embedded`].

use sedan_gql_embedded::{GraphStore, QueryResult, SedanGql};

use crate::seed::GRAPH;

/// Run an arbitrary GQL query against the given store.
pub fn run(store: &dyn GraphStore, query: &str) -> QueryResult {
    let engine = SedanGql::new();
    engine
        .execute(store, GRAPH, query)
        .unwrap_or_else(|e| panic!("gql query failed: {e}\nquery: {query}"))
}

/// GQL: friends of Alice, ordered by name.
pub const Q_ALICE_FRIENDS: &str =
    "MATCH (p:Person {name: 'Alice'})-[:FRIEND]->(f:Person) RETURN f.name AS name ORDER BY name";

/// GQL: people aged > 30, ordered by name.
pub const Q_OVER_30: &str = "MATCH (p:Person) WHERE p.age > 30 RETURN p.name AS name ORDER BY name";
