use super::*;
use crate::storage::GraphStore;
use crate::storage::{EdgeId, NodeId};
use crate::types::Value;
use indexmap::IndexMap;
use std::fs;
use std::time::Duration;

fn test_config(dir: &std::path::Path) -> PersistentConfig {
    PersistentConfig {
        data_dir: dir.to_path_buf(),
        snapshot_interval: Duration::from_secs(3600), // long interval; we trigger manually
        sync_mode: SyncMode::Immediate,
    }
}

#[test]
fn basic_write_read_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();

    store.create_graph("g").unwrap();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    let nid = store
        .create_node("g", vec!["Person".to_string()], props)
        .unwrap();

    let node = store.get_node("g", nid).unwrap().unwrap();
    assert_eq!(node.labels, vec!["Person"]);
    assert_eq!(node.properties["name"], Value::Str("Alice".into()));

    // Create edge
    let mut props2 = IndexMap::new();
    props2.insert("since".to_string(), Value::Int(2020));
    let nid2 = store
        .create_node("g", vec!["Person".to_string()], IndexMap::new())
        .unwrap();
    let eid = store
        .create_edge("g", nid, nid2, "KNOWS".to_string(), props2)
        .unwrap();
    let edge = store.get_edge("g", eid).unwrap().unwrap();
    assert_eq!(edge.rel_type, "KNOWS");
    assert_eq!(edge.properties["since"], Value::Int(2020));
}

#[test]
fn recovery_after_reopen() {
    let dir = tempfile::tempdir().unwrap();

    // Phase 1: write data
    {
        let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
        store.create_graph("g").unwrap();
        let mut props = IndexMap::new();
        props.insert("name".to_string(), Value::Str("Bob".into()));
        store
            .create_node("g", vec!["Person".to_string()], props)
            .unwrap();
        let nid2 = store
            .create_node("g", vec!["Animal".to_string()], IndexMap::new())
            .unwrap();
        assert_eq!(nid2.0, 2);
        // Drop flushes
    }

    // Phase 2: reopen and verify
    {
        let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
        assert!(store.graph_exists("g").unwrap());
        let node = store.get_node("g", NodeId(1)).unwrap().unwrap();
        assert_eq!(node.properties["name"], Value::Str("Bob".into()));
        assert_eq!(node.labels, vec!["Person"]);

        let node2 = store.get_node("g", NodeId(2)).unwrap().unwrap();
        assert_eq!(node2.labels, vec!["Animal"]);

        let stats = store.graph_stats("g").unwrap();
        assert_eq!(stats.node_count, 2);
    }
}

#[test]
fn wal_replay_ordering() {
    let dir = tempfile::tempdir().unwrap();

    // Write a graph, add nodes and an edge, modify properties, delete node
    {
        let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
        store.create_graph("g").unwrap();

        let n1 = store
            .create_node("g", vec!["A".to_string()], IndexMap::new())
            .unwrap();
        let n2 = store
            .create_node("g", vec!["B".to_string()], IndexMap::new())
            .unwrap();
        let _e = store
            .create_edge("g", n1, n2, "R".to_string(), IndexMap::new())
            .unwrap();

        let mut update = IndexMap::new();
        update.insert("x".to_string(), Value::Int(42));
        store.set_node_properties("g", n1, update).unwrap();

        // Delete n2 (detach to remove edge too)
        store.delete_node("g", n2, true).unwrap();
    }

    // Replay and verify
    {
        let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
        let node = store.get_node("g", NodeId(1)).unwrap().unwrap();
        assert_eq!(node.properties["x"], Value::Int(42));

        // n2 was deleted
        assert!(store.get_node("g", NodeId(2)).unwrap().is_none());

        let stats = store.graph_stats("g").unwrap();
        assert_eq!(stats.node_count, 1);
        assert_eq!(stats.edge_count, 0);
    }
}

#[test]
fn snapshot_and_wal_truncation() {
    let dir = tempfile::tempdir().unwrap();

    {
        let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
        store.create_graph("g").unwrap();
        let mut props = IndexMap::new();
        props.insert("v".to_string(), Value::Int(1));
        store
            .create_node("g", vec!["X".to_string()], props)
            .unwrap();

        // Force snapshot (truncates WAL)
        store.force_snapshot().unwrap();

        // WAL should now be empty
        let wal_content = fs::read_to_string(dir.path().join("wal.jsonl")).unwrap();
        assert!(wal_content.is_empty(), "WAL should be empty after snapshot");

        // Snapshot should exist
        assert!(dir.path().join("snapshot.json").exists());

        // Write more data after snapshot
        let mut props2 = IndexMap::new();
        props2.insert("v".to_string(), Value::Int(2));
        store
            .create_node("g", vec!["Y".to_string()], props2)
            .unwrap();
    }

    // Reopen: should recover from snapshot + WAL
    {
        let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
        let n1 = store.get_node("g", NodeId(1)).unwrap().unwrap();
        assert_eq!(n1.properties["v"], Value::Int(1));
        assert_eq!(n1.labels, vec!["X"]);

        let n2 = store.get_node("g", NodeId(2)).unwrap().unwrap();
        assert_eq!(n2.properties["v"], Value::Int(2));
        assert_eq!(n2.labels, vec!["Y"]);
    }
}

#[test]
fn edge_operations_persist() {
    let dir = tempfile::tempdir().unwrap();

    {
        let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
        store.create_graph("g").unwrap();
        let n1 = store.create_node("g", vec![], IndexMap::new()).unwrap();
        let n2 = store.create_node("g", vec![], IndexMap::new()).unwrap();

        let mut ep = IndexMap::new();
        ep.insert("weight".to_string(), Value::Float(3.25));
        let eid = store
            .create_edge("g", n1, n2, "LINK".to_string(), ep)
            .unwrap();

        // Update edge properties
        let mut update = IndexMap::new();
        update.insert("color".to_string(), Value::Str("red".into()));
        store.set_edge_properties("g", eid, update).unwrap();
    }

    {
        let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
        let edge = store.get_edge("g", EdgeId(3)).unwrap().unwrap();
        assert_eq!(edge.rel_type, "LINK");
        assert_eq!(edge.properties["weight"], Value::Float(3.25));
        assert_eq!(edge.properties["color"], Value::Str("red".into()));
    }
}

#[test]
fn index_operations_persist() {
    let dir = tempfile::tempdir().unwrap();

    {
        let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
        store.create_graph("g").unwrap();
        store.create_index("g", "Person", "name").unwrap();

        let indexes = store.list_indexes("g").unwrap();
        assert_eq!(indexes.len(), 1);
    }

    {
        let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
        let indexes = store.list_indexes("g").unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0].label, "Person");
        assert_eq!(indexes[0].property, "name");
    }
}

#[test]
fn label_operations_persist() {
    let dir = tempfile::tempdir().unwrap();

    {
        let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
        store.create_graph("g").unwrap();
        let nid = store
            .create_node("g", vec!["A".to_string()], IndexMap::new())
            .unwrap();
        store
            .add_node_labels("g", nid, &["B".to_string(), "C".to_string()])
            .unwrap();
        store
            .remove_node_labels("g", nid, &["A".to_string()])
            .unwrap();
    }

    {
        let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
        let node = store.get_node("g", NodeId(1)).unwrap().unwrap();
        assert!(!node.labels.contains(&"A".to_string()));
        assert!(node.labels.contains(&"B".to_string()));
        assert!(node.labels.contains(&"C".to_string()));
    }
}

#[test]
fn empty_store_opens_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let store = PersistentGraphStore::open(test_config(dir.path())).unwrap();
    assert_eq!(store.list_graphs().unwrap(), Vec::<String>::new());
}
