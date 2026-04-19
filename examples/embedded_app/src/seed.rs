//! Builds a small social-network graph in an [`InMemoryGraphStore`].
//!
//! Schema:
//! - `(:Person {name, age, city})` — 10 people
//! - `(:City {name})` — 3 cities
//! - `(:Person)-[:FRIEND]->(:Person)` — directed friendship edges
//! - `(:Person)-[:LIVES_IN]->(:City)` — one per person

use indexmap::IndexMap;
use sedan_cypher_embedded::{GraphStore, InMemoryGraphStore, Value};

/// Graph name used by every subcommand in this example.
pub const GRAPH: &str = "social";

/// Build the demo graph and return the store.
pub fn build() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph(GRAPH).expect("create graph");

    let cities = ["Seattle", "Portland", "Vancouver"];
    let mut city_ids = std::collections::HashMap::new();
    for name in cities {
        let id = store
            .create_node(
                GRAPH,
                vec!["City".into()],
                IndexMap::from([("name".into(), Value::Str(name.into()))]),
            )
            .expect("create city");
        city_ids.insert(name, id);
    }

    let people = [
        ("Alice", 30, "Seattle"),
        ("Bob", 25, "Seattle"),
        ("Carol", 40, "Portland"),
        ("Dave", 35, "Portland"),
        ("Eve", 28, "Vancouver"),
        ("Frank", 45, "Seattle"),
        ("Grace", 33, "Portland"),
        ("Heidi", 27, "Vancouver"),
        ("Ivan", 50, "Seattle"),
        ("Judy", 22, "Vancouver"),
    ];
    let mut person_ids = std::collections::HashMap::new();
    for (name, age, city) in people {
        let id = store
            .create_node(
                GRAPH,
                vec!["Person".into()],
                IndexMap::from([
                    ("name".into(), Value::Str(name.into())),
                    ("age".into(), Value::Int(age)),
                    ("city".into(), Value::Str(city.into())),
                ]),
            )
            .expect("create person");
        person_ids.insert(name, id);

        store
            .create_edge(
                GRAPH,
                id,
                city_ids[city],
                "LIVES_IN".into(),
                IndexMap::new(),
            )
            .expect("create lives_in");
    }

    let friendships = [
        ("Alice", "Bob"),
        ("Alice", "Carol"),
        ("Alice", "Dave"),
        ("Bob", "Eve"),
        ("Bob", "Frank"),
        ("Carol", "Grace"),
        ("Dave", "Heidi"),
        ("Eve", "Ivan"),
        ("Frank", "Judy"),
        ("Grace", "Alice"),
        ("Heidi", "Bob"),
        ("Ivan", "Carol"),
    ];
    for (from, to) in friendships {
        store
            .create_edge(
                GRAPH,
                person_ids[from],
                person_ids[to],
                "FRIEND".into(),
                IndexMap::new(),
            )
            .expect("create friend");
    }

    store
}
