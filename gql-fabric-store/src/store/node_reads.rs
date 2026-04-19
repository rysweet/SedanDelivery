// Node and adjacency read operations.
//
// Each `*_impl` function is invoked from the `GraphStore` trait impl in
// `mod.rs`. The actual Cypher strings come from the pure builders in
// `nodes.rs`.

use serde_json::{json, Value as JValue};

use cypher_core::error::StorageError;
use cypher_core::storage::{AdjEntry, EdgeId, NodeData, NodeId, RangeQuery};

use crate::parsing::rows_to_adj;
use crate::sanitize::{range_to_cypher, sanitize_label};
use crate::FabricGraphStore;

use super::nodes;

pub(crate) fn get_node_impl(
    store: &FabricGraphStore,
    graph: &str,
    id: NodeId,
) -> Result<Option<NodeData>, StorageError> {
    let rows = store.query(
        &nodes::get_node_query(),
        json!({ "graph": graph, "id": id.0 }),
    )?;
    rows.first()
        .map(|r| store.row_to_node_data(r))
        .unwrap_or(Ok(None))
}

pub(crate) fn get_nodes_batch_impl(
    store: &FabricGraphStore,
    graph: &str,
    ids: &[NodeId],
) -> Result<Vec<Option<NodeData>>, StorageError> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let id_list: JValue = ids.iter().map(|n| n.0).collect::<Vec<_>>().into();
    let rows = store.query(
        &nodes::get_nodes_batch_query(),
        json!({ "graph": graph, "ids": id_list }),
    )?;
    rows.iter().map(|r| store.row_to_node_data(r)).collect()
}

pub(crate) fn find_nodes_by_label_impl(
    store: &FabricGraphStore,
    graph: &str,
    label: &str,
) -> Result<Vec<NodeId>, StorageError> {
    let sanitized = sanitize_label(label);
    if sanitized.is_empty() {
        return Err(StorageError::Backend {
            msg: format!(
                "invalid label: sanitize_label produced empty string from {:?}",
                label
            ),
        });
    }
    let rows = store.query(
        &nodes::find_nodes_by_label_query(&sanitized),
        json!({ "graph": graph }),
    )?;
    Ok(rows
        .iter()
        .filter_map(|r| r.first()?.as_u64().map(NodeId))
        .collect::<Vec<_>>())
}

pub(crate) fn find_nodes_by_label_and_property_impl(
    store: &FabricGraphStore,
    graph: &str,
    label: &str,
    property: &str,
    range: RangeQuery<'_>,
) -> Result<Vec<NodeId>, StorageError> {
    let label = sanitize_label(label);
    let prop = sanitize_label(property);
    let (predicate, params) = range_to_cypher(&prop, range);
    let mut p = params;
    p["graph"] = json!(graph);
    let rows = store.query(
        &nodes::find_nodes_by_label_and_property_query(&label, &predicate),
        p,
    )?;
    Ok(rows
        .iter()
        .filter_map(|r| r.first()?.as_u64().map(NodeId))
        .collect::<Vec<_>>())
}

pub(crate) fn all_node_ids_impl(
    store: &FabricGraphStore,
    graph: &str,
) -> Result<Vec<NodeId>, StorageError> {
    let rows = store.query(&nodes::all_node_ids_query(), json!({ "graph": graph }))?;
    Ok(rows
        .iter()
        .filter_map(|r| r.first()?.as_u64().map(NodeId))
        .collect::<Vec<_>>())
}

pub(crate) fn all_label_names_impl(
    store: &FabricGraphStore,
    graph: &str,
) -> Result<Vec<String>, StorageError> {
    let rows = store.query(&nodes::all_label_names_query(), json!({ "graph": graph }))?;
    let mut seen = std::collections::HashSet::new();
    for row in &rows {
        if let Some(s) = row.first().and_then(|v| v.as_str()) {
            if let Ok(labels) = serde_json::from_str::<Vec<String>>(s) {
                for l in labels {
                    seen.insert(l);
                }
            }
        }
    }
    Ok(seen.into_iter().collect())
}

// ── Adjacency reads ─────────────────────────────────────────────────────────

pub(crate) fn out_edges_impl(
    store: &FabricGraphStore,
    graph: &str,
    node_id: NodeId,
) -> Result<Vec<AdjEntry>, StorageError> {
    let rows = store.query(
        &nodes::out_edges_query(),
        json!({ "graph": graph, "id": node_id.0 }),
    )?;
    rows_to_adj(rows)
}

pub(crate) fn in_edges_impl(
    store: &FabricGraphStore,
    graph: &str,
    node_id: NodeId,
) -> Result<Vec<AdjEntry>, StorageError> {
    let rows = store.query(
        &nodes::in_edges_query(),
        json!({ "graph": graph, "id": node_id.0 }),
    )?;
    rows_to_adj(rows)
}

pub(crate) fn out_edges_by_type_impl(
    store: &FabricGraphStore,
    graph: &str,
    node_id: NodeId,
    rel_types: &[&str],
) -> Result<Vec<AdjEntry>, StorageError> {
    if rel_types.is_empty() {
        return out_edges_impl(store, graph, node_id);
    }
    let rt_list: JValue = rel_types
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .into();
    let rows = store.query(
        &nodes::out_edges_by_type_query(),
        json!({ "graph": graph, "id": node_id.0, "types": rt_list }),
    )?;
    rows_to_adj(rows)
}

pub(crate) fn in_edges_by_type_impl(
    store: &FabricGraphStore,
    graph: &str,
    node_id: NodeId,
    rel_types: &[&str],
) -> Result<Vec<AdjEntry>, StorageError> {
    if rel_types.is_empty() {
        return in_edges_impl(store, graph, node_id);
    }
    let rt_list: JValue = rel_types
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .into();
    let rows = store.query(
        &nodes::in_edges_by_type_query(),
        json!({ "graph": graph, "id": node_id.0, "types": rt_list }),
    )?;
    rows_to_adj(rows)
}

pub(crate) fn out_edge_ids_impl(
    store: &FabricGraphStore,
    graph: &str,
    node_id: NodeId,
) -> Result<Vec<EdgeId>, StorageError> {
    let rows = store.query(
        &nodes::out_edge_ids_query(),
        json!({ "graph": graph, "id": node_id.0 }),
    )?;
    Ok(rows
        .iter()
        .filter_map(|r| r.first()?.as_u64().map(EdgeId))
        .collect::<Vec<_>>())
}

pub(crate) fn in_edge_ids_impl(
    store: &FabricGraphStore,
    graph: &str,
    node_id: NodeId,
) -> Result<Vec<EdgeId>, StorageError> {
    let rows = store.query(
        &nodes::in_edge_ids_query(),
        json!({ "graph": graph, "id": node_id.0 }),
    )?;
    Ok(rows
        .iter()
        .filter_map(|r| r.first()?.as_u64().map(EdgeId))
        .collect::<Vec<_>>())
}
