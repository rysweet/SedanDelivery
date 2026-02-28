//! Change log types for cross-backend synchronisation.
//!
//! # How it works
//!
//! `ChangeLogStore<S>` wraps any `GraphStore` (typically `InMemoryGraphStore`)
//! and intercepts every write operation.  After a write succeeds on the inner
//! store the operation is recorded as a `ChangeEvent` in an in-process pending
//! queue.
//!
//! The Redis module command handler drains the queue after each successful
//! `GQL.MUTATE` / `GQL.CREATE-GRAPH` / `GQL.DELETE-GRAPH` command and appends
//! one Valkey stream entry per event via `ctx.call("XADD", ...)`.  This is a
//! module-internal call — no TCP socket, no deadlock.
//!
//! `sedandelivery-fabric-sync` (a separate process, optional) consumes the stream via
//! `XREADGROUP` and replays each event against `FabricGraphStore`.
//!
//! # Property encoding
//!
//! `ChangeEvent` stores properties as `serde_json::Map<String, serde_json::Value>`
//! using the canonical tagged-string encoding defined in `crate::serde_value`.
//! This keeps `ChangeEvent` trivially `Serialize + Deserialize` via serde,
//! while preserving full round-trip fidelity for temporal, spatial, and list
//! values.
//!
//! # Enabling the changelog
//!
//! Set the `GQL_FABRIC_CHANGELOG=1` environment variable (or set any
//! `GQL_FABRIC_ENDPOINT` / `GQL_FABRIC_*` variables) before loading the module.
//! When disabled, `drain_events()` returns an empty `Vec` and the module
//! command handlers skip the `XADD` calls entirely — zero overhead.
//!
//! # Stream key
//!
//! All events are written to the Valkey stream `__sd2:changelog`.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Map as JsonMap;
use serde_json::Value as JsonValue;

use crate::error::StorageError;
use crate::serde_value::{json_to_value, value_to_json};
use crate::storage::{
    AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor, NodeData, NodeId,
    RangeQuery,
};
use crate::types::Value;

// ── Stream key ────────────────────────────────────────────────────────────────

/// Valkey stream key where change events are published.
pub const CHANGELOG_STREAM: &str = "__sd2:changelog";

// ── Property map helpers ──────────────────────────────────────────────────────

/// Convert a `HashMap<String, Value>` to a JSON object map using the
/// canonical tagged-string encoding.  Values that fail serialisation
/// (non-finite floats) are logged and dropped — the storage layer already
/// rejects these, so this only triggers on truly corrupted data.
fn props_to_json_map(props: &HashMap<String, Value>) -> JsonMap<String, JsonValue> {
    let mut m = JsonMap::with_capacity(props.len());
    for (k, v) in props {
        match value_to_json(v) {
            Ok(jv) => {
                m.insert(k.clone(), jv);
            }
            Err(e) => {
                tracing::warn!(key = %k, "changelog: dropping non-serialisable property: {e}");
            }
        }
    }
    m
}

/// Convert a JSON object map back to `HashMap<String, Value>`.
/// Values that cannot be decoded are logged and skipped.
pub fn json_map_to_props(m: &JsonMap<String, JsonValue>) -> HashMap<String, Value> {
    m.iter()
        .filter_map(|(k, v)| match json_to_value(v) {
            Some(val) => Some((k.clone(), val)),
            None => {
                tracing::warn!(key = %k, "changelog: dropping non-decodable property");
                None
            }
        })
        .collect()
}

// ── ChangeEvent ───────────────────────────────────────────────────────────────

/// A single graph write operation recorded in the change log.
///
/// Every variant captures exactly the arguments needed to replay the
/// operation idempotently on another `GraphStore` (e.g. `FabricGraphStore`).
/// For `CreateNode` / `CreateEdge` the ID assigned by the primary store is
/// included so the secondary can use `_sd2_id` / `_sd2_eid` injection to
/// keep IDs consistent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ChangeEvent {
    // ── Graph-level ───────────────────────────────────────────────────────────
    CreateGraph {
        name: String,
    },
    DeleteGraph {
        name: String,
    },

    // ── Node writes ───────────────────────────────────────────────────────────
    CreateNode {
        graph: String,
        id: u64,
        labels: Vec<String>,
        properties: JsonMap<String, JsonValue>,
    },
    SetNodeProperties {
        graph: String,
        id: u64,
        properties: JsonMap<String, JsonValue>,
    },
    ReplaceNodeProperties {
        graph: String,
        id: u64,
        properties: JsonMap<String, JsonValue>,
    },
    RemoveNodeProperty {
        graph: String,
        id: u64,
        key: String,
    },
    AddNodeLabels {
        graph: String,
        id: u64,
        labels: Vec<String>,
    },
    RemoveNodeLabels {
        graph: String,
        id: u64,
        labels: Vec<String>,
    },
    DeleteNode {
        graph: String,
        id: u64,
        detach: bool,
    },

    // ── Edge writes ───────────────────────────────────────────────────────────
    CreateEdge {
        graph: String,
        id: u64,
        src: u64,
        dst: u64,
        rel_type: String,
        properties: JsonMap<String, JsonValue>,
    },
    SetEdgeProperties {
        graph: String,
        id: u64,
        properties: JsonMap<String, JsonValue>,
    },
    ReplaceEdgeProperties {
        graph: String,
        id: u64,
        properties: JsonMap<String, JsonValue>,
    },
    RemoveEdgeProperty {
        graph: String,
        id: u64,
        key: String,
    },
    DeleteEdge {
        graph: String,
        id: u64,
    },

    // ── Index management ──────────────────────────────────────────────────────
    CreateIndex {
        graph: String,
        label: String,
        property: String,
    },
    DropIndex {
        graph: String,
        label: String,
        property: String,
    },
}

// ── ChangeLogStore ────────────────────────────────────────────────────────────

/// Wraps any [`GraphStore`] and records every successful write as a
/// [`ChangeEvent`] in an in-process pending queue.
///
/// The caller is responsible for draining the queue (via [`drain_events`])
/// and forwarding the events to a durable sink (e.g. a Valkey stream).
///
/// [`drain_events`]: ChangeLogStore::drain_events
pub struct ChangeLogStore<S> {
    inner: S,
    pending: Mutex<Vec<ChangeEvent>>,
}

impl<S> ChangeLogStore<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            pending: Mutex::new(Vec::new()),
        }
    }

    /// Push an event onto the pending queue.
    ///
    /// A poisoned mutex is ignored rather than panicked — the module must not
    /// crash a Redis/Valkey process because of a changelog accounting error.
    fn push(&self, event: ChangeEvent) {
        match self.pending.lock() {
            Ok(mut v) => v.push(event),
            Err(_) => {
                tracing::error!("changelog: pending mutex poisoned — event dropped");
            }
        }
    }

    /// Drain all accumulated events.
    ///
    /// Call this after a successful command to retrieve events for forwarding
    /// to the changelog stream.  On failure / panic, call this to discard any
    /// partial events so they don't leak into the next command's batch.
    pub fn drain_events(&self) -> Vec<ChangeEvent> {
        match self.pending.lock() {
            Ok(mut v) => std::mem::take(&mut *v),
            Err(_) => {
                tracing::error!("changelog: pending mutex poisoned — returning empty event list");
                Vec::new()
            }
        }
    }
}

// ── GraphStore implementation ─────────────────────────────────────────────────

impl<S: GraphStore> GraphStore for ChangeLogStore<S> {
    // ── Reads: straight delegation ────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        self.inner.get_node(graph, id)
    }

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        self.inner.get_edge(graph, id)
    }

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        self.inner.get_nodes_batch(graph, ids)
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        self.inner.get_edges_batch(graph, ids)
    }

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        self.inner.find_nodes_by_label(graph, label)
    }

    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        self.inner
            .find_nodes_by_label_and_property(graph, label, property, range)
    }

    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.inner.find_edges_by_type(graph, rel_type)
    }

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.inner.out_edges(graph, node_id)
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.inner.in_edges(graph, node_id)
    }

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.inner.out_edges_by_type(graph, node_id, rel_types)
    }

    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.inner.in_edges_by_type(graph, node_id, rel_types)
    }

    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.inner.out_edge_ids(graph, node_id)
    }

    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.inner.in_edge_ids(graph, node_id)
    }

    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        self.inner.all_node_ids(graph)
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.inner.all_edge_ids(graph)
    }

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        self.inner.all_label_names(graph)
    }

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        self.inner.list_indexes(graph)
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        self.inner.graph_exists(name)
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        self.inner.list_graphs()
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        self.inner.graph_stats(graph)
    }

    // ── Writes: delegate then record event ───────────────────────────────────

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: HashMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        let id = self
            .inner
            .create_node(graph, labels.clone(), properties.clone())?;
        self.push(ChangeEvent::CreateNode {
            graph: graph.to_string(),
            id: id.0,
            labels,
            properties: props_to_json_map(&properties),
        });
        Ok(id)
    }

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: HashMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        let id = self
            .inner
            .create_edge(graph, src, dst, rel_type.clone(), properties.clone())?;
        self.push(ChangeEvent::CreateEdge {
            graph: graph.to_string(),
            id: id.0,
            src: src.0,
            dst: dst.0,
            rel_type,
            properties: props_to_json_map(&properties),
        });
        Ok(id)
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.inner
            .set_node_properties(graph, id, properties.clone())?;
        self.push(ChangeEvent::SetNodeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json_map(&properties),
        });
        Ok(())
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.inner
            .replace_node_properties(graph, id, properties.clone())?;
        self.push(ChangeEvent::ReplaceNodeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json_map(&properties),
        });
        Ok(())
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.inner
            .set_edge_properties(graph, id, properties.clone())?;
        self.push(ChangeEvent::SetEdgeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json_map(&properties),
        });
        Ok(())
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.inner
            .replace_edge_properties(graph, id, properties.clone())?;
        self.push(ChangeEvent::ReplaceEdgeProperties {
            graph: graph.to_string(),
            id: id.0,
            properties: props_to_json_map(&properties),
        });
        Ok(())
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        self.inner.remove_node_property(graph, id, key)?;
        self.push(ChangeEvent::RemoveNodeProperty {
            graph: graph.to_string(),
            id: id.0,
            key: key.to_string(),
        });
        Ok(())
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        self.inner.remove_edge_property(graph, id, key)?;
        self.push(ChangeEvent::RemoveEdgeProperty {
            graph: graph.to_string(),
            id: id.0,
            key: key.to_string(),
        });
        Ok(())
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.inner.add_node_labels(graph, id, labels)?;
        self.push(ChangeEvent::AddNodeLabels {
            graph: graph.to_string(),
            id: id.0,
            labels: labels.to_vec(),
        });
        Ok(())
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.inner.remove_node_labels(graph, id, labels)?;
        self.push(ChangeEvent::RemoveNodeLabels {
            graph: graph.to_string(),
            id: id.0,
            labels: labels.to_vec(),
        });
        Ok(())
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        self.inner.delete_node(graph, id, detach)?;
        self.push(ChangeEvent::DeleteNode {
            graph: graph.to_string(),
            id: id.0,
            detach,
        });
        Ok(())
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.inner.delete_edge(graph, id)?;
        self.push(ChangeEvent::DeleteEdge {
            graph: graph.to_string(),
            id: id.0,
        });
        Ok(())
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.inner.create_index(graph, label, property)?;
        self.push(ChangeEvent::CreateIndex {
            graph: graph.to_string(),
            label: label.to_string(),
            property: property.to_string(),
        });
        Ok(())
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.inner.drop_index(graph, label, property)?;
        self.push(ChangeEvent::DropIndex {
            graph: graph.to_string(),
            label: label.to_string(),
            property: property.to_string(),
        });
        Ok(())
    }

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        self.inner.create_graph(name)?;
        self.push(ChangeEvent::CreateGraph {
            name: name.to_string(),
        });
        Ok(())
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        self.inner.delete_graph(name)?;
        self.push(ChangeEvent::DeleteGraph {
            name: name.to_string(),
        });
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::InMemoryGraphStore;

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

        let mut props = HashMap::new();
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
        let result = store.create_node("nonexistent", vec![], HashMap::new());
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
}
