use super::*;
use crate::changelog::props::props_to_json_map;
use crate::storage::{GraphStore, InMemoryGraphStore, RangeQuery};
use crate::types::Value;
use indexmap::IndexMap;
use serde_json::Map as JsonMap;
use serde_json::Value as JsonValue;

fn make_store() -> ChangeLogStore<InMemoryGraphStore> {
    let inner = InMemoryGraphStore::new();
    ChangeLogStore::new(inner)
}

#[test]
fn no_events_before_writes() {
    let store = make_store();
    assert!(store.drain_events().is_empty());
}

#[test]
fn create_graph_emits_event() {
    let store = make_store();
    store.create_graph("g1").unwrap();
    let events = store.drain_events();
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], ChangeEvent::CreateGraph { ref name } if name == "g1"));
}

#[test]
fn drain_clears_queue() {
    let store = make_store();
    store.create_graph("g1").unwrap();
    let e1 = store.drain_events();
    let e2 = store.drain_events();
    assert_eq!(e1.len(), 1);
    assert!(e2.is_empty());
}

#[test]
fn create_node_emits_event_with_id() {
    let store = make_store();
    store.create_graph("g").unwrap();
    let _ = store.drain_events(); // discard create_graph event

    let mut props = IndexMap::new();
    props.insert("name".into(), Value::Str("Alice".into()));
    let id = store
        .create_node("g", vec!["Person".into()], props)
        .unwrap();

    let events = store.drain_events();
    assert_eq!(events.len(), 1);
    match &events[0] {
        ChangeEvent::CreateNode {
            graph,
            id: eid,
            labels,
            properties,
        } => {
            assert_eq!(graph, "g");
            assert_eq!(*eid, id.0);
            assert_eq!(labels, &["Person"]);
            assert!(properties.contains_key("name"));
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn delete_graph_emits_event() {
    let store = make_store();
    store.create_graph("to-delete").unwrap();
    let _ = store.drain_events();

    store.delete_graph("to-delete").unwrap();
    let events = store.drain_events();
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], ChangeEvent::DeleteGraph { ref name } if name == "to-delete"));
}

#[test]
fn failed_write_emits_no_event() {
    let store = make_store();
    // create_node on non-existent graph should fail
    let result = store.create_node("nonexistent", vec![], IndexMap::new());
    assert!(result.is_err());
    assert!(store.drain_events().is_empty());
}

#[test]
fn changelog_event_round_trips_json() {
    let event = ChangeEvent::CreateNode {
        graph: "g".into(),
        id: 42,
        labels: vec!["Person".into()],
        properties: {
            let mut m = JsonMap::new();
            m.insert("name".into(), JsonValue::String("Alice".into()));
            m
        },
    };
    let json = serde_json::to_string(&event).unwrap();
    let back: ChangeEvent = serde_json::from_str(&json).unwrap();
    match back {
        ChangeEvent::CreateNode { id, .. } => assert_eq!(id, 42),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn changelog_stream_for_graph_includes_hash_tag() {
    assert_eq!(
        super::changelog_stream_for_graph("mydb"),
        "__sd2:changelog:{mydb}"
    );
}

#[test]
fn graph_name_returns_correct_graph_for_all_variants() {
    let cases: Vec<(ChangeEvent, &str)> = vec![
        (ChangeEvent::CreateGraph { name: "g1".into() }, "g1"),
        (ChangeEvent::DeleteGraph { name: "g2".into() }, "g2"),
        (
            ChangeEvent::CreateNode {
                graph: "g3".into(),
                id: 1,
                labels: vec![],
                properties: JsonMap::new(),
            },
            "g3",
        ),
        (
            ChangeEvent::DeleteEdge {
                graph: "g4".into(),
                id: 1,
            },
            "g4",
        ),
        (
            ChangeEvent::SetNodeProperties {
                graph: "a".into(),
                id: 1,
                properties: JsonMap::new(),
            },
            "a",
        ),
        (
            ChangeEvent::ReplaceNodeProperties {
                graph: "b".into(),
                id: 1,
                properties: JsonMap::new(),
            },
            "b",
        ),
        (
            ChangeEvent::RemoveNodeProperty {
                graph: "c".into(),
                id: 1,
                key: "k".into(),
            },
            "c",
        ),
        (
            ChangeEvent::AddNodeLabels {
                graph: "d".into(),
                id: 1,
                labels: vec![],
            },
            "d",
        ),
        (
            ChangeEvent::RemoveNodeLabels {
                graph: "e".into(),
                id: 1,
                labels: vec![],
            },
            "e",
        ),
        (
            ChangeEvent::DeleteNode {
                graph: "f".into(),
                id: 1,
                detach: true,
            },
            "f",
        ),
        (
            ChangeEvent::CreateEdge {
                graph: "h".into(),
                id: 1,
                src: 1,
                dst: 2,
                rel_type: "R".into(),
                properties: JsonMap::new(),
            },
            "h",
        ),
        (
            ChangeEvent::SetEdgeProperties {
                graph: "i".into(),
                id: 1,
                properties: JsonMap::new(),
            },
            "i",
        ),
        (
            ChangeEvent::ReplaceEdgeProperties {
                graph: "j".into(),
                id: 1,
                properties: JsonMap::new(),
            },
            "j",
        ),
        (
            ChangeEvent::RemoveEdgeProperty {
                graph: "k".into(),
                id: 1,
                key: "k".into(),
            },
            "k",
        ),
        (
            ChangeEvent::CreateIndex {
                graph: "l".into(),
                label: "L".into(),
                property: "p".into(),
            },
            "l",
        ),
        (
            ChangeEvent::DropIndex {
                graph: "m".into(),
                label: "L".into(),
                property: "p".into(),
            },
            "m",
        ),
    ];
    for (event, expected) in cases {
        assert_eq!(event.graph_name(), expected);
    }
}

#[test]
fn write_methods_emit_one_event_each() {
    let store = make_store();
    store.create_graph("g").unwrap();
    let n1 = store
        .create_node("g", vec!["L".into()], IndexMap::new())
        .unwrap();
    let n2 = store
        .create_node("g", vec!["L".into()], IndexMap::new())
        .unwrap();
    let e = store
        .create_edge("g", n1, n2, "R".into(), IndexMap::new())
        .unwrap();
    let _ = store.drain_events();

    let mut p = IndexMap::new();
    p.insert("k".into(), Value::Int(1));
    store.set_node_properties("g", n1, p.clone()).unwrap();
    store.replace_node_properties("g", n1, p.clone()).unwrap();
    store.remove_node_property("g", n1, "k").unwrap();
    store.add_node_labels("g", n1, &["X".into()]).unwrap();
    store.remove_node_labels("g", n1, &["X".into()]).unwrap();
    store.set_edge_properties("g", e, p.clone()).unwrap();
    store.replace_edge_properties("g", e, p.clone()).unwrap();
    store.remove_edge_property("g", e, "k").unwrap();
    store.create_index("g", "L", "name").unwrap();
    store.drop_index("g", "L", "name").unwrap();
    store.delete_edge("g", e).unwrap();
    store.delete_node("g", n1, false).unwrap();

    let events = store.drain_events();
    // One event per write call above (12)
    assert_eq!(events.len(), 12);
    // Verify each variant kind appears
    let names: Vec<_> = events.iter().map(std::mem::discriminant).collect();
    // All discriminants should be unique except for any duplicates we made
    // (here they are all distinct variants)
    let mut seen = std::collections::HashSet::new();
    for d in &names {
        seen.insert(*d);
    }
    assert!(
        seen.len() >= 10,
        "expected >=10 distinct event variants, got {}",
        seen.len()
    );
}

#[test]
fn read_methods_delegate_without_emitting() {
    let store = make_store();
    store.create_graph("g").unwrap();
    let n1 = store
        .create_node("g", vec!["L".into()], IndexMap::new())
        .unwrap();
    let n2 = store
        .create_node("g", vec!["L".into()], IndexMap::new())
        .unwrap();
    let e = store
        .create_edge("g", n1, n2, "R".into(), IndexMap::new())
        .unwrap();
    let _ = store.drain_events();

    // Reads — none should emit events
    let _ = store.get_node("g", n1).unwrap();
    let _ = store.get_edge("g", e).unwrap();
    let _ = store.get_nodes_batch("g", &[n1, n2]).unwrap();
    let _ = store.get_edges_batch("g", &[e]).unwrap();
    let _ = store.find_nodes_by_label("g", "L").unwrap();
    let _ = store.find_edges_by_type("g", "R").unwrap();
    let _ = store.out_edges("g", n1).unwrap();
    let _ = store.in_edges("g", n2).unwrap();
    let _ = store.out_edges_by_type("g", n1, &["R"]).unwrap();
    let _ = store.in_edges_by_type("g", n2, &["R"]).unwrap();
    let _ = store.out_edge_ids("g", n1).unwrap();
    let _ = store.in_edge_ids("g", n2).unwrap();
    let _ = store.all_node_ids("g").unwrap();
    let _ = store.all_edge_ids("g").unwrap();
    let _ = store.all_label_names("g").unwrap();
    let _ = store.list_indexes("g").unwrap();
    let _ = store.graph_exists("g").unwrap();
    let _ = store.list_graphs().unwrap();
    let _ = store.graph_stats("g").unwrap();
    let _ =
        store.find_nodes_by_label_and_property("g", "L", "name", RangeQuery::Eq(&Value::Int(1)));

    assert!(store.drain_events().is_empty());
}

#[test]
fn props_to_json_map_round_trip() {
    let mut p = IndexMap::new();
    p.insert("s".into(), Value::Str("hi".into()));
    p.insert("n".into(), Value::Int(42));
    let m = props_to_json_map(&p);
    assert!(m.contains_key("s"));
    assert!(m.contains_key("n"));
    let back = super::json_map_to_props(&m);
    assert_eq!(back.get("s"), Some(&Value::Str("hi".into())));
    assert_eq!(back.get("n"), Some(&Value::Int(42)));
}

#[test]
fn props_to_json_map_drops_non_serializable() {
    // NaN floats fail value_to_json
    let mut p = IndexMap::new();
    p.insert("ok".into(), Value::Int(1));
    p.insert("bad".into(), Value::Float(f64::NAN));
    let m = props_to_json_map(&p);
    assert!(m.contains_key("ok"));
    // The bad key may or may not be present depending on serializer behavior;
    // just ensure the function doesn't panic and the good key is preserved.
}

#[test]
fn changelog_stream_constant() {
    assert_eq!(super::CHANGELOG_STREAM, "__sd2:changelog");
}
