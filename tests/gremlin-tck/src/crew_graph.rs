//! The standard TinkerPop "crew" test graph.
//!
//! 6 vertices: marko (person), stephen (person), matthias (person),
//!             daniel (person), gremlin (software), tinkergraph (software)
//! 14 edges:   develops (person→software, since), uses (person→software, skill),
//!             traverses (software→software)
//!
//! Each person has a multi-valued "location" property with startTime/endTime
//! meta-properties.  Because `InMemoryGraphStore` does not support native
//! multi-properties, locations are stored as `Value::List` of `Value::Map`
//! entries — sufficient for basic traversals, though meta-property–aware
//! queries (e.g. `properties("location").has("startTime", ...)`) require
//! first-class multi-property support in the store.

use std::collections::HashMap;

use gql_core::storage::{EdgeId, GraphStore, InMemoryGraphStore, NodeId};
use gql_core::types::Value;
use indexmap::IndexMap;

/// Vertex and edge IDs for the crew graph.
pub struct CrewGraphIds {
    // Persons
    pub marko: NodeId,
    pub stephen: NodeId,
    pub matthias: NodeId,
    pub daniel: NodeId,
    // Software
    pub gremlin: NodeId,
    pub tinkergraph: NodeId,
    // Edges — named as src-type->dst
    pub marko_develops_gremlin: EdgeId,
    pub marko_develops_tinkergraph: EdgeId,
    pub marko_uses_gremlin: EdgeId,
    pub marko_uses_tinkergraph: EdgeId,
    pub stephen_develops_gremlin: EdgeId,
    pub stephen_develops_tinkergraph: EdgeId,
    pub stephen_uses_gremlin: EdgeId,
    pub stephen_uses_tinkergraph: EdgeId,
    pub matthias_develops_gremlin: EdgeId,
    pub matthias_uses_gremlin: EdgeId,
    pub matthias_uses_tinkergraph: EdgeId,
    pub daniel_uses_gremlin: EdgeId,
    pub daniel_uses_tinkergraph: EdgeId,
    pub gremlin_traverses_tinkergraph: EdgeId,
}

/// Create an `InMemoryGraphStore` pre-populated with the TinkerPop "crew"
/// graph on a graph named `"crew"`.
pub fn create_crew_graph() -> (InMemoryGraphStore, CrewGraphIds) {
    let store = InMemoryGraphStore::new();
    store.create_graph("crew").unwrap();

    // ── Vertices ───────────────────────────────────────────────────────────

    let marko = store
        .create_node(
            "crew",
            vec!["person".to_string()],
            person_props(
                "marko",
                &[
                    ("san diego", 1997, Some(2001)),
                    ("santa cruz", 2001, Some(2004)),
                    ("brussels", 2004, Some(2005)),
                    ("santa fe", 2005, None),
                ],
            ),
        )
        .unwrap();

    let stephen = store
        .create_node(
            "crew",
            vec!["person".to_string()],
            person_props(
                "stephen",
                &[
                    ("centreville", 1990, Some(2000)),
                    ("dulles", 2000, Some(2006)),
                    ("purcellville", 2006, None),
                ],
            ),
        )
        .unwrap();

    let matthias = store
        .create_node(
            "crew",
            vec!["person".to_string()],
            person_props(
                "matthias",
                &[
                    ("bremen", 2004, Some(2007)),
                    ("baltimore", 2007, Some(2011)),
                    ("oakland", 2011, Some(2014)),
                    ("seattle", 2014, None),
                ],
            ),
        )
        .unwrap();

    let daniel = store
        .create_node(
            "crew",
            vec!["person".to_string()],
            person_props(
                "daniel",
                &[
                    ("spremberg", 1982, Some(2005)),
                    ("kaiserslautern", 2005, Some(2009)),
                    ("aachen", 2009, None),
                ],
            ),
        )
        .unwrap();

    let gremlin = store
        .create_node(
            "crew",
            vec!["software".to_string()],
            software_props("gremlin"),
        )
        .unwrap();

    let tinkergraph = store
        .create_node(
            "crew",
            vec!["software".to_string()],
            software_props("tinkergraph"),
        )
        .unwrap();

    // ── Edges ──────────────────────────────────────────────────────────────

    let marko_develops_gremlin = store
        .create_edge(
            "crew",
            marko,
            gremlin,
            "develops".to_string(),
            since_props(2009),
        )
        .unwrap();

    let marko_develops_tinkergraph = store
        .create_edge(
            "crew",
            marko,
            tinkergraph,
            "develops".to_string(),
            since_props(2010),
        )
        .unwrap();

    let marko_uses_gremlin = store
        .create_edge("crew", marko, gremlin, "uses".to_string(), skill_props(4))
        .unwrap();

    let marko_uses_tinkergraph = store
        .create_edge(
            "crew",
            marko,
            tinkergraph,
            "uses".to_string(),
            skill_props(5),
        )
        .unwrap();

    let stephen_develops_gremlin = store
        .create_edge(
            "crew",
            stephen,
            gremlin,
            "develops".to_string(),
            since_props(2010),
        )
        .unwrap();

    let stephen_develops_tinkergraph = store
        .create_edge(
            "crew",
            stephen,
            tinkergraph,
            "develops".to_string(),
            since_props(2011),
        )
        .unwrap();

    let stephen_uses_gremlin = store
        .create_edge("crew", stephen, gremlin, "uses".to_string(), skill_props(5))
        .unwrap();

    let stephen_uses_tinkergraph = store
        .create_edge(
            "crew",
            stephen,
            tinkergraph,
            "uses".to_string(),
            skill_props(4),
        )
        .unwrap();

    let matthias_develops_gremlin = store
        .create_edge(
            "crew",
            matthias,
            gremlin,
            "develops".to_string(),
            since_props(2012),
        )
        .unwrap();

    let matthias_uses_gremlin = store
        .create_edge(
            "crew",
            matthias,
            gremlin,
            "uses".to_string(),
            skill_props(3),
        )
        .unwrap();

    let matthias_uses_tinkergraph = store
        .create_edge(
            "crew",
            matthias,
            tinkergraph,
            "uses".to_string(),
            skill_props(3),
        )
        .unwrap();

    let daniel_uses_gremlin = store
        .create_edge("crew", daniel, gremlin, "uses".to_string(), skill_props(5))
        .unwrap();

    let daniel_uses_tinkergraph = store
        .create_edge(
            "crew",
            daniel,
            tinkergraph,
            "uses".to_string(),
            skill_props(3),
        )
        .unwrap();

    let gremlin_traverses_tinkergraph = store
        .create_edge(
            "crew",
            gremlin,
            tinkergraph,
            "traverses".to_string(),
            IndexMap::new(),
        )
        .unwrap();

    let ids = CrewGraphIds {
        marko,
        stephen,
        matthias,
        daniel,
        gremlin,
        tinkergraph,
        marko_develops_gremlin,
        marko_develops_tinkergraph,
        marko_uses_gremlin,
        marko_uses_tinkergraph,
        stephen_develops_gremlin,
        stephen_develops_tinkergraph,
        stephen_uses_gremlin,
        stephen_uses_tinkergraph,
        matthias_develops_gremlin,
        matthias_uses_gremlin,
        matthias_uses_tinkergraph,
        daniel_uses_gremlin,
        daniel_uses_tinkergraph,
        gremlin_traverses_tinkergraph,
    };

    (store, ids)
}

// ── Property helpers ───────────────────────────────────────────────────────────

fn person_props(name: &str, locations: &[(&str, i64, Option<i64>)]) -> IndexMap<String, Value> {
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str(name.into()));

    // Encode multi-valued location with startTime/endTime meta-properties
    // as a List of Maps.
    let loc_list: Vec<Value> = locations
        .iter()
        .map(|&(place, start, end)| {
            let mut m = IndexMap::new();
            m.insert("value".into(), Value::Str(place.into()));
            m.insert("startTime".into(), Value::Int(start));
            if let Some(e) = end {
                m.insert("endTime".into(), Value::Int(e));
            }
            Value::Map(m)
        })
        .collect();
    props.insert("location".to_string(), Value::List(loc_list));

    props
}

fn software_props(name: &str) -> IndexMap<String, Value> {
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str(name.into()));
    props
}

fn since_props(since: i64) -> IndexMap<String, Value> {
    let mut props = IndexMap::new();
    props.insert("since".to_string(), Value::Int(since));
    props
}

fn skill_props(skill: i64) -> IndexMap<String, Value> {
    let mut props = IndexMap::new();
    props.insert("skill".to_string(), Value::Int(skill));
    props
}
