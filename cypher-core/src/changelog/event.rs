//! [`ChangeEvent`] enum: the wire-level record of a single graph write.

use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};

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

impl ChangeEvent {
    /// Return the graph name associated with this event, used to determine
    /// which per-graph changelog stream (`__sd2:changelog:{graph}`) to write to.
    pub fn graph_name(&self) -> &str {
        match self {
            ChangeEvent::CreateGraph { name } | ChangeEvent::DeleteGraph { name } => name,
            ChangeEvent::CreateNode { graph, .. }
            | ChangeEvent::SetNodeProperties { graph, .. }
            | ChangeEvent::ReplaceNodeProperties { graph, .. }
            | ChangeEvent::RemoveNodeProperty { graph, .. }
            | ChangeEvent::AddNodeLabels { graph, .. }
            | ChangeEvent::RemoveNodeLabels { graph, .. }
            | ChangeEvent::DeleteNode { graph, .. }
            | ChangeEvent::CreateEdge { graph, .. }
            | ChangeEvent::SetEdgeProperties { graph, .. }
            | ChangeEvent::ReplaceEdgeProperties { graph, .. }
            | ChangeEvent::RemoveEdgeProperty { graph, .. }
            | ChangeEvent::DeleteEdge { graph, .. }
            | ChangeEvent::CreateIndex { graph, .. }
            | ChangeEvent::DropIndex { graph, .. } => graph,
        }
    }
}
