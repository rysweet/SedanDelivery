//! Gremlin traversals through [`sedan_gremlin_embedded`].

use sedan_gremlin_embedded::{GraphStore, GremlinEngine, QueryResult};

use crate::seed::GRAPH;

/// Run an arbitrary Gremlin traversal against the given store.
pub fn run(store: &dyn GraphStore, query: &str) -> QueryResult {
    let engine = GremlinEngine::new();
    engine
        .execute(store, GRAPH, query)
        .unwrap_or_else(|e| panic!("gremlin query failed: {e}\nquery: {query}"))
}

/// Gremlin: friends of Alice.
pub const Q_ALICE_FRIENDS: &str = "g.V().has('Person','name','Alice').out('FRIEND').values('name')";

/// Gremlin: people aged > 30.
pub const Q_OVER_30: &str = "g.V().hasLabel('Person').has('age', gt(30)).values('name')";
