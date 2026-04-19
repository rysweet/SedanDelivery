//! Cypher → Bolt value encoding.
//!
//! Converts cypher-core values (`Value`, `NodeData`, `EdgeData`, paths) into
//! Bolt protocol [`BoltValue`]s. Submodules are split by type family:
//!
//! - [`scalars`] — null, bool, int, float, string, bytes, list, map dispatch
//! - [`entities`] — Node and Relationship (Bolt tags 0x4E, 0x52)
//! - [`path`] — Path encoding (tag 0x50) with unbound relationships
//! - [`temporal`] — Date/Time/DateTime/Duration encoders
//! - [`spatial`] — Point2D/Point3D encoders
//!
//! Decoding (Bolt → cypher-core) lives in sibling modules
//! `crate::bridge::temporal` and `crate::bridge::spatial`.

use std::collections::HashMap;

use cypher_core::storage::{EdgeData, EdgeId, GraphStore, NodeData, NodeId};

use crate::packstream::value::BoltValue;

mod entities;
mod path;
mod scalars;
mod spatial;
mod temporal;

#[cfg(test)]
mod tests;

pub use entities::{edge_to_bolt, node_to_bolt};
pub(crate) use entities::{edge_to_bolt_cached, node_to_bolt_cached};
pub use path::path_to_bolt;
pub(crate) use path::path_to_bolt_cached;
pub use scalars::{props_to_bolt_map, scalar_value_to_bolt};

/// Convert a `u64` storage ID to an `i64` for the Bolt wire format.
/// Returns an error message and `BoltValue::Null` if the ID exceeds `i64::MAX`.
pub(super) fn u64_to_bolt_int(id: u64, context: &str) -> BoltValue {
    match i64::try_from(id) {
        Ok(v) => BoltValue::Int(v),
        Err(_) => {
            tracing::error!(id, context, "ID exceeds i64::MAX, cannot encode for Bolt");
            BoltValue::Null
        }
    }
}

pub(crate) struct BoltGraphCache {
    nodes: HashMap<NodeId, Option<NodeData>>,
    edges: HashMap<EdgeId, Option<EdgeData>>,
}

impl BoltGraphCache {
    pub(crate) fn prefetch(
        store: &dyn GraphStore,
        graph: &str,
        node_ids: &[NodeId],
        edge_ids: &[EdgeId],
    ) -> Self {
        let mut nodes = HashMap::with_capacity(node_ids.len());
        if !node_ids.is_empty() {
            match store.get_nodes_batch(graph, node_ids) {
                Ok(batch) => {
                    for (node_id, data) in node_ids.iter().copied().zip(batch) {
                        nodes.insert(node_id, data);
                    }
                }
                Err(e) => {
                    tracing::error!(graph = graph, error = %e, "bolt prefetch: node batch lookup failed");
                }
            }
        }

        let mut edges = HashMap::with_capacity(edge_ids.len());
        if !edge_ids.is_empty() {
            match store.get_edges_batch(graph, edge_ids) {
                Ok(batch) => {
                    for (edge_id, data) in edge_ids.iter().copied().zip(batch) {
                        edges.insert(edge_id, data);
                    }
                }
                Err(e) => {
                    tracing::error!(graph = graph, error = %e, "bolt prefetch: edge batch lookup failed");
                }
            }
        }

        Self { nodes, edges }
    }

    pub(super) fn node(&self, id: NodeId) -> Option<&Option<NodeData>> {
        self.nodes.get(&id)
    }

    pub(super) fn edge(&self, id: EdgeId) -> Option<&Option<EdgeData>> {
        self.edges.get(&id)
    }
}
