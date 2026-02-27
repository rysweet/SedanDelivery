//! Neighbor traversal endpoint.
//!
//! GET /graphs/{name}/neighbors/{nodeId}?direction=out&types=KNOWS,WORKS_AT&limit=50

use std::collections::HashSet;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};

use gql_core::storage::{validate_graph_name, GraphStore, NodeId};

use crate::error::ApiError;
use crate::models::NeighborParams;
use crate::serialize::{edge_to_json, node_to_json};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/graphs/{name}/neighbors/{node_id}", get(neighbors_handler))
}

async fn neighbors_handler(
    State(state): State<AppState>,
    Path((name, node_id_str)): Path<(String, String)>,
    Query(params): Query<NeighborParams>,
) -> Result<Json<crate::models::NeighborResponse>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;

    let node_id: NodeId = node_id_str
        .parse::<u64>()
        .map(NodeId)
        .map_err(|_| ApiError::bad_request(format!("invalid node id: '{node_id_str}'")))?;

    let store = state
        .get_store(&name)
        .ok_or_else(|| ApiError::not_found(format!("graph '{name}' not found")))?;

    // Verify the node exists.
    store
        .get_node(&name, node_id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("node {node_id_str} not found")))?;

    // Parse relationship type filter.
    let type_filter: Vec<&str> = params
        .types
        .as_deref()
        .map(|s| s.split(',').map(str::trim).filter(|t| !t.is_empty()).collect())
        .unwrap_or_default();

    // Determine direction.
    let direction = params.direction.as_deref().unwrap_or("both");

    let adj_entries = match direction {
        "out" => {
            if type_filter.is_empty() {
                store.out_edges(&name, node_id).map_err(ApiError::from)?
            } else {
                store
                    .out_edges_by_type(&name, node_id, &type_filter)
                    .map_err(ApiError::from)?
            }
        }
        "in" => {
            if type_filter.is_empty() {
                store.in_edges(&name, node_id).map_err(ApiError::from)?
            } else {
                store
                    .in_edges_by_type(&name, node_id, &type_filter)
                    .map_err(ApiError::from)?
            }
        }
        _ => {
            // "both" or any other value
            let mut entries = if type_filter.is_empty() {
                store.out_edges(&name, node_id).map_err(ApiError::from)?
            } else {
                store
                    .out_edges_by_type(&name, node_id, &type_filter)
                    .map_err(ApiError::from)?
            };
            let inbound = if type_filter.is_empty() {
                store.in_edges(&name, node_id).map_err(ApiError::from)?
            } else {
                store
                    .in_edges_by_type(&name, node_id, &type_filter)
                    .map_err(ApiError::from)?
            };
            entries.extend(inbound);
            entries
        }
    };

    // Apply limit.
    let limit = params.limit;
    let limited: Vec<_> = adj_entries.into_iter().take(limit).collect();

    // Collect unique neighbor node IDs and unique edge IDs.
    let mut neighbor_ids: Vec<NodeId> = Vec::new();
    let mut seen_nodes: HashSet<u64> = HashSet::new();
    let mut edge_ids = Vec::new();
    let mut seen_edges: HashSet<u64> = HashSet::new();

    for entry in &limited {
        if seen_nodes.insert(entry.neighbor_id.0) {
            neighbor_ids.push(entry.neighbor_id);
        }
        if seen_edges.insert(entry.edge_id.0) {
            edge_ids.push(entry.edge_id);
        }
    }

    // Batch-fetch nodes and edges.
    let node_batch = store
        .get_nodes_batch(&name, &neighbor_ids)
        .map_err(ApiError::from)?;
    let edge_batch = store
        .get_edges_batch(&name, &edge_ids)
        .map_err(ApiError::from)?;

    let nodes: Vec<serde_json::Value> = node_batch
        .iter()
        .flatten()
        .map(node_to_json)
        .collect();

    let edges: Vec<serde_json::Value> = edge_batch
        .iter()
        .flatten()
        .map(edge_to_json)
        .collect();

    Ok(Json(crate::models::NeighborResponse { nodes, edges }))
}
