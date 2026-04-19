//! Path encoding (Bolt structure tag 0x50).
//!
//! Encodes a sequence of nodes and edges into the Bolt path representation:
//! a list of unique nodes, a list of unbound relationships, and an index
//! sequence describing traversal order and direction.

use std::collections::HashMap;

use cypher_core::storage::{EdgeId, GraphStore, NodeId};

use crate::packstream::value::BoltValue;
use crate::protocol::tags;

use super::entities::{node_to_bolt, node_to_bolt_cached};
use super::scalars::props_to_bolt_map;
use super::{u64_to_bolt_int, BoltGraphCache};

/// Convert a path to a Bolt Structure (tag 0x50).
///
/// Fields: [nodes: List<Node>, rels: List<UnboundRelationship>, indices: List<Int>]
///
/// Unbound relationships use tag 0x72 with fields: [id, type, properties].
///
/// The index sequence encodes the traversal order: for each step, a positive
/// relationship index (1-based) means forward traversal, a negative index means
/// reverse. The subsequent node index (0-based into unique-nodes list) follows.
pub fn path_to_bolt(
    node_ids: &[NodeId],
    edge_ids: &[EdgeId],
    store: &dyn GraphStore,
    graph: &str,
) -> BoltValue {
    // Build the unique-nodes list (preserving first-occurrence order).
    let mut unique_nodes: Vec<NodeId> = Vec::new();
    let mut node_index_map: HashMap<NodeId, usize> = HashMap::new();
    for &nid in node_ids {
        if let std::collections::hash_map::Entry::Vacant(e) = node_index_map.entry(nid) {
            e.insert(unique_nodes.len());
            unique_nodes.push(nid);
        }
    }

    let bolt_unique_nodes: Vec<BoltValue> = unique_nodes
        .iter()
        .map(|&nid| node_to_bolt(nid, store, graph))
        .collect();

    // Build unbound relationships (tag 0x72: id, type, properties).
    let mut unbound_rels: Vec<BoltValue> = Vec::new();
    let mut edge_rel_index: Vec<usize> = Vec::new();
    let mut seen_edges: HashMap<EdgeId, usize> = HashMap::new();
    for &eid in edge_ids {
        if let Some(&idx) = seen_edges.get(&eid) {
            edge_rel_index.push(idx);
        } else {
            let idx = unbound_rels.len();
            seen_edges.insert(eid, idx);
            edge_rel_index.push(idx);

            let rel = match store.get_edge(graph, eid) {
                Ok(Some(data)) => BoltValue::Structure {
                    tag: tags::TAG_UNBOUND_RELATIONSHIP,
                    fields: vec![
                        u64_to_bolt_int(eid.0, "path_to_bolt:rel_id"),
                        BoltValue::String(data.rel_type.clone()),
                        props_to_bolt_map(&data.properties),
                    ],
                },
                Ok(None) => {
                    tracing::warn!(
                        edge_id = eid.0,
                        graph = graph,
                        "path_to_bolt: edge not found in store, using empty relationship"
                    );
                    BoltValue::Structure {
                        tag: tags::TAG_UNBOUND_RELATIONSHIP,
                        fields: vec![
                            u64_to_bolt_int(eid.0, "path_to_bolt:rel_id"),
                            BoltValue::String(String::new()),
                            BoltValue::Map(vec![]),
                        ],
                    }
                }
                Err(e) => {
                    tracing::error!(edge_id = eid.0, graph = graph, error = %e, "path_to_bolt: store lookup failed for edge, using empty relationship");
                    BoltValue::Structure {
                        tag: tags::TAG_UNBOUND_RELATIONSHIP,
                        fields: vec![
                            u64_to_bolt_int(eid.0, "path_to_bolt:rel_id"),
                            BoltValue::String(String::new()),
                            BoltValue::Map(vec![]),
                        ],
                    }
                }
            };
            unbound_rels.push(rel);
        }
    }

    // Build the index sequence.
    // For each step i: edge_ids[i] connects node_ids[i] -> node_ids[i+1].
    // Relationship index is 1-based into unbound_rels.
    // If the edge's src matches node_ids[i], we traverse forward (+rel_idx).
    // If the edge's dst matches node_ids[i], we traverse backward (-rel_idx).
    let mut indices: Vec<BoltValue> = Vec::new();
    for i in 0..edge_ids.len() {
        let rel_1based = (edge_rel_index[i] as i64) + 1;
        let from_node = node_ids[i];

        // Determine direction by checking edge src/dst against the from-node.
        let rel_index = match store.get_edge(graph, edge_ids[i]) {
            Ok(Some(data)) => {
                if data.src == from_node {
                    rel_1based
                } else {
                    -rel_1based
                }
            }
            Ok(None) => {
                tracing::warn!(edge_id = edge_ids[i].0, graph = graph, "path_to_bolt: edge not found when determining direction, defaulting to forward");
                rel_1based
            }
            Err(e) => {
                tracing::error!(edge_id = edge_ids[i].0, graph = graph, error = %e, "path_to_bolt: store lookup failed when determining direction, defaulting to forward");
                rel_1based
            }
        };
        indices.push(BoltValue::Int(rel_index));

        let to_node = node_ids[i + 1];
        let node_idx = match node_index_map.get(&to_node) {
            Some(&idx) => idx as i64,
            None => {
                tracing::warn!(
                    node_id = to_node.0,
                    "node_index_map missing to_node in path encoding, falling back to index 0"
                );
                0
            }
        };
        indices.push(BoltValue::Int(node_idx));
    }

    BoltValue::Structure {
        tag: tags::TAG_PATH,
        fields: vec![
            BoltValue::List(bolt_unique_nodes),
            BoltValue::List(unbound_rels),
            BoltValue::List(indices),
        ],
    }
}

pub(crate) fn path_to_bolt_cached(
    node_ids: &[NodeId],
    edge_ids: &[EdgeId],
    cache: &BoltGraphCache,
    store: &dyn GraphStore,
    graph: &str,
) -> BoltValue {
    let mut unique_nodes: Vec<NodeId> = Vec::new();
    let mut node_index_map: HashMap<NodeId, usize> = HashMap::new();
    for &nid in node_ids {
        if let std::collections::hash_map::Entry::Vacant(entry) = node_index_map.entry(nid) {
            entry.insert(unique_nodes.len());
            unique_nodes.push(nid);
        }
    }

    let bolt_unique_nodes: Vec<BoltValue> = unique_nodes
        .iter()
        .map(|&nid| node_to_bolt_cached(nid, cache, store, graph))
        .collect();

    let mut unbound_rels: Vec<BoltValue> = Vec::new();
    let mut edge_rel_index: Vec<usize> = Vec::new();
    let mut seen_edges: HashMap<EdgeId, usize> = HashMap::new();
    for &eid in edge_ids {
        if let Some(&idx) = seen_edges.get(&eid) {
            edge_rel_index.push(idx);
            continue;
        }

        let idx = unbound_rels.len();
        seen_edges.insert(eid, idx);
        edge_rel_index.push(idx);
        let rel = match cache.edge(eid) {
            Some(Some(data)) => BoltValue::Structure {
                tag: tags::TAG_UNBOUND_RELATIONSHIP,
                fields: vec![
                    u64_to_bolt_int(eid.0, "path_to_bolt_cached:rel_id"),
                    BoltValue::String(data.rel_type.clone()),
                    props_to_bolt_map(&data.properties),
                ],
            },
            Some(None) => {
                tracing::warn!(
                    edge_id = eid.0,
                    graph = graph,
                    "path_to_bolt_cached: edge not found in store, using empty relationship"
                );
                BoltValue::Structure {
                    tag: tags::TAG_UNBOUND_RELATIONSHIP,
                    fields: vec![
                        u64_to_bolt_int(eid.0, "path_to_bolt_cached:rel_id"),
                        BoltValue::String(String::new()),
                        BoltValue::Map(vec![]),
                    ],
                }
            }
            None => return path_to_bolt(node_ids, edge_ids, store, graph),
        };
        unbound_rels.push(rel);
    }

    let mut indices: Vec<BoltValue> = Vec::new();
    for i in 0..edge_ids.len() {
        let rel_1based = (edge_rel_index[i] as i64) + 1;
        let from_node = node_ids[i];
        let rel_index = match cache.edge(edge_ids[i]) {
            Some(Some(data)) => {
                if data.src == from_node {
                    rel_1based
                } else {
                    -rel_1based
                }
            }
            Some(None) => {
                tracing::warn!(
                    edge_id = edge_ids[i].0,
                    graph = graph,
                    "path_to_bolt_cached: edge not found when determining direction, defaulting to forward"
                );
                rel_1based
            }
            None => return path_to_bolt(node_ids, edge_ids, store, graph),
        };
        indices.push(BoltValue::Int(rel_index));

        let to_node = node_ids[i + 1];
        let node_idx = match node_index_map.get(&to_node) {
            Some(&idx) => idx as i64,
            None => {
                tracing::warn!(
                    node_id = to_node.0,
                    "node_index_map missing to_node in path encoding, falling back to index 0"
                );
                0
            }
        };
        indices.push(BoltValue::Int(node_idx));
    }

    BoltValue::Structure {
        tag: tags::TAG_PATH,
        fields: vec![
            BoltValue::List(bolt_unique_nodes),
            BoltValue::List(unbound_rels),
            BoltValue::List(indices),
        ],
    }
}
