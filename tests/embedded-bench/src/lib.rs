//! Shared, deterministic graph fixtures for the embedded reliability bench
//! suite. All seed data is generated from a fixed integer schedule so that
//! results are stable across runs and across engines.
//!
//! The 1000-node fixture builds a `:Person` graph with:
//!   * `age = i % 100` (so `WHERE age > 21` selects ~78% of nodes)
//!   * `name = "p_<i>"`
//!   * a chain edge `p_i -[:KNOWS]-> p_{i+1}` (so a 2-hop traversal is dense)
//!   * extra fan-out edges from a deterministic schedule
//!
//! Each engine gets its own bench file so results are reported as
//! `cypher/<bench>`, `gql/<bench>`, `gremlin/<bench>`.

use cypher_core::storage::{GraphStore, InMemoryGraphStore};
use cypher_core::types::Value;
use indexmap::IndexMap;

/// Default node count for the standard fixture. The bench specs in the
/// PR description target a 1000-node graph.
pub const NODES: usize = 1000;

/// Number of extra (non-chain) edges added to the fixture.
pub const EXTRA_EDGES: usize = 1000;

/// Graph name used by every bench in the suite.
pub const GRAPH: &str = "g";

/// Build the standard 1000-node `:Person` fixture using direct `GraphStore`
/// writes — this avoids attributing seed cost to whichever engine happens
/// to be under test.
pub fn seeded_store() -> InMemoryGraphStore {
    seeded_store_with(NODES, EXTRA_EDGES)
}

/// Build a fixture with a caller-supplied node and extra-edge count.
pub fn seeded_store_with(nodes: usize, extra_edges: usize) -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph(GRAPH).expect("create_graph");

    let mut ids = Vec::with_capacity(nodes);
    for i in 0..nodes {
        let mut props = IndexMap::new();
        props.insert("name".into(), Value::Str(format!("p_{i}").into()));
        props.insert("age".into(), Value::Int((i % 100) as i64));
        let id = store
            .create_node(GRAPH, vec!["Person".into()], props)
            .expect("create_node");
        ids.push(id);
    }

    // Chain edges — guarantees a non-trivial result for `MATCH (a)-[*2]->(c)`.
    for i in 0..nodes.saturating_sub(1) {
        store
            .create_edge(GRAPH, ids[i], ids[i + 1], "KNOWS".into(), IndexMap::new())
            .expect("create_edge chain");
    }

    // Deterministic fan-out edges (no RNG — seeded by index).
    for i in 0..extra_edges {
        let a = i % nodes;
        let b = (i.wrapping_mul(2654435761)) % nodes; // Knuth multiplicative hash
        if a == b {
            continue;
        }
        store
            .create_edge(GRAPH, ids[a], ids[b], "KNOWS".into(), IndexMap::new())
            .expect("create_edge fan-out");
    }

    store
}

/// 100 distinct integer parameter values used by the `parameter_binding`
/// bench across all three engines.
pub fn param_schedule() -> Vec<i64> {
    (0..100).map(|i| (i * 7 + 3) % 100).collect()
}
