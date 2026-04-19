//! Node and Relationship encoding (Bolt structure tags 0x4E and 0x52).

use cypher_core::storage::{EdgeId, GraphStore, NodeId};

use crate::packstream::value::BoltValue;
use crate::protocol::tags;

use super::scalars::props_to_bolt_map;
use super::{u64_to_bolt_int, BoltGraphCache};

/// Convert a node to a Bolt Structure (tag 0x4E).
///
/// Fields: [id: Int, labels: List<String>, properties: Map]
///
/// Returns `BoltValue::Null` if the node is not found in the store.
pub fn node_to_bolt(id: NodeId, store: &dyn GraphStore, graph: &str) -> BoltValue {
    let node_data = match store.get_node(graph, id) {
        Ok(Some(data)) => data,
        Ok(None) => return BoltValue::Null,
        Err(e) => {
            tracing::error!(node_id = id.0, graph = graph, error = %e, "node_to_bolt: store lookup failed");
            return BoltValue::Null;
        }
    };

    let labels = BoltValue::List(
        node_data
            .labels
            .iter()
            .map(|l| BoltValue::String(l.clone()))
            .collect(),
    );

    let props = props_to_bolt_map(&node_data.properties);

    BoltValue::Structure {
        tag: tags::TAG_NODE,
        fields: vec![u64_to_bolt_int(id.0, "node_to_bolt"), labels, props],
    }
}

pub(crate) fn node_to_bolt_cached(
    id: NodeId,
    cache: &BoltGraphCache,
    store: &dyn GraphStore,
    graph: &str,
) -> BoltValue {
    match cache.node(id) {
        Some(Some(data)) => {
            let labels = BoltValue::List(
                data.labels
                    .iter()
                    .map(|label| BoltValue::String(label.clone()))
                    .collect(),
            );
            let props = props_to_bolt_map(&data.properties);
            BoltValue::Structure {
                tag: tags::TAG_NODE,
                fields: vec![u64_to_bolt_int(id.0, "node_to_bolt_cached"), labels, props],
            }
        }
        Some(None) => BoltValue::Null,
        None => node_to_bolt(id, store, graph),
    }
}

/// Convert an edge to a Bolt Structure (tag 0x52).
///
/// Fields: [id: Int, startNodeId: Int, endNodeId: Int, type: String, properties: Map]
///
/// Returns `BoltValue::Null` if the edge is not found in the store.
pub fn edge_to_bolt(id: EdgeId, store: &dyn GraphStore, graph: &str) -> BoltValue {
    let edge_data = match store.get_edge(graph, id) {
        Ok(Some(data)) => data,
        Ok(None) => return BoltValue::Null,
        Err(e) => {
            tracing::error!(edge_id = id.0, graph = graph, error = %e, "edge_to_bolt: store lookup failed");
            return BoltValue::Null;
        }
    };

    let props = props_to_bolt_map(&edge_data.properties);

    BoltValue::Structure {
        tag: tags::TAG_RELATIONSHIP,
        fields: vec![
            u64_to_bolt_int(id.0, "edge_to_bolt:id"),
            u64_to_bolt_int(edge_data.src.0, "edge_to_bolt:src"),
            u64_to_bolt_int(edge_data.dst.0, "edge_to_bolt:dst"),
            BoltValue::String(edge_data.rel_type.clone()),
            props,
        ],
    }
}

pub(crate) fn edge_to_bolt_cached(
    id: EdgeId,
    cache: &BoltGraphCache,
    store: &dyn GraphStore,
    graph: &str,
) -> BoltValue {
    match cache.edge(id) {
        Some(Some(data)) => {
            let props = props_to_bolt_map(&data.properties);
            BoltValue::Structure {
                tag: tags::TAG_RELATIONSHIP,
                fields: vec![
                    u64_to_bolt_int(id.0, "edge_to_bolt_cached:id"),
                    u64_to_bolt_int(data.src.0, "edge_to_bolt_cached:src"),
                    u64_to_bolt_int(data.dst.0, "edge_to_bolt_cached:dst"),
                    BoltValue::String(data.rel_type.clone()),
                    props,
                ],
            }
        }
        Some(None) => BoltValue::Null,
        None => edge_to_bolt(id, store, graph),
    }
}
