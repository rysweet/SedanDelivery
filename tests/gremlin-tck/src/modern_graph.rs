//! The standard TinkerPop "modern" test graph.
//!
//! 6 vertices: marko (person, 29), vadas (person, 27), lop (software, java),
//!             josh (person, 32), ripple (software, java), peter (person, 35)
//! 6 edges:    marko-[knows,0.5]->vadas, marko-[knows,1.0]->josh,
//!             marko-[created,0.4]->lop, josh-[created,1.0]->ripple,
//!             josh-[created,0.4]->lop, peter-[created,0.2]->lop

use std::collections::HashMap;

use gql_core::storage::{GraphStore, InMemoryGraphStore, NodeId};
use gql_core::types::Value;

/// Vertex IDs returned after building the modern graph, for convenient
/// reference in tests that need to assert on specific nodes.
pub struct ModernGraphIds {
    pub marko: NodeId,
    pub vadas: NodeId,
    pub lop: NodeId,
    pub josh: NodeId,
    pub ripple: NodeId,
    pub peter: NodeId,
}

/// Create an `InMemoryGraphStore` pre-populated with the TinkerPop "modern"
/// graph on a graph named `"modern"`.
pub fn create_modern_graph() -> (InMemoryGraphStore, ModernGraphIds) {
    let store = InMemoryGraphStore::new();
    store.create_graph("modern").unwrap();

    // ── Vertices ───────────────────────────────────────────────────────────

    let marko = store
        .create_node(
            "modern",
            vec!["person".to_string()],
            person_props("marko", 29),
        )
        .unwrap();

    let vadas = store
        .create_node(
            "modern",
            vec!["person".to_string()],
            person_props("vadas", 27),
        )
        .unwrap();

    let lop = store
        .create_node(
            "modern",
            vec!["software".to_string()],
            software_props("lop", "java"),
        )
        .unwrap();

    let josh = store
        .create_node(
            "modern",
            vec!["person".to_string()],
            person_props("josh", 32),
        )
        .unwrap();

    let ripple = store
        .create_node(
            "modern",
            vec!["software".to_string()],
            software_props("ripple", "java"),
        )
        .unwrap();

    let peter = store
        .create_node(
            "modern",
            vec!["person".to_string()],
            person_props("peter", 35),
        )
        .unwrap();

    // ── Edges ──────────────────────────────────────────────────────────────

    store
        .create_edge(
            "modern",
            marko,
            vadas,
            "knows".to_string(),
            weight_props(0.5),
        )
        .unwrap();

    store
        .create_edge(
            "modern",
            marko,
            josh,
            "knows".to_string(),
            weight_props(1.0),
        )
        .unwrap();

    store
        .create_edge(
            "modern",
            marko,
            lop,
            "created".to_string(),
            weight_props(0.4),
        )
        .unwrap();

    store
        .create_edge(
            "modern",
            josh,
            ripple,
            "created".to_string(),
            weight_props(1.0),
        )
        .unwrap();

    store
        .create_edge(
            "modern",
            josh,
            lop,
            "created".to_string(),
            weight_props(0.4),
        )
        .unwrap();

    store
        .create_edge(
            "modern",
            peter,
            lop,
            "created".to_string(),
            weight_props(0.2),
        )
        .unwrap();

    let ids = ModernGraphIds {
        marko,
        vadas,
        lop,
        josh,
        ripple,
        peter,
    };

    (store, ids)
}

// ── Property helpers ───────────────────────────────────────────────────────────

fn person_props(name: &str, age: i64) -> HashMap<String, Value> {
    let mut props = HashMap::new();
    props.insert("name".to_string(), Value::Str(name.into()));
    props.insert("age".to_string(), Value::Int(age));
    props
}

fn software_props(name: &str, lang: &str) -> HashMap<String, Value> {
    let mut props = HashMap::new();
    props.insert("name".to_string(), Value::Str(name.into()));
    props.insert("lang".to_string(), Value::Str(lang.into()));
    props
}

fn weight_props(weight: f64) -> HashMap<String, Value> {
    let mut props = HashMap::new();
    props.insert("weight".to_string(), Value::Float(weight));
    props
}
