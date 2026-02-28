//! Schema introspection endpoint.
//!
//! GET /graphs/{name}/schema
//!
//! Returns label counts and relationship-type counts derived from a full
//! node / edge scan via the storage layer.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};

use gql_core::storage::{validate_graph_name, GraphStore};

use crate::error::ApiError;
use crate::models::{LabelInfo, RelTypeInfo, SchemaResponse};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/graphs/{name}/schema", get(schema_handler))
}

async fn schema_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<SchemaResponse>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;

    let store = state
        .get_store(&name)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("graph '{name}' not found")))?;

    // ── Collect label counts ──────────────────────────────────────────────────

    let label_counts = collect_label_counts(store.as_ref(), &name)?;
    let mut labels: Vec<LabelInfo> = label_counts
        .into_iter()
        .map(|(label, count)| LabelInfo { name: label, count })
        .collect();
    labels.sort_by(|a, b| a.name.cmp(&b.name));

    // ── Collect relationship-type counts ──────────────────────────────────────

    let reltype_counts = collect_reltype_counts(store.as_ref(), &name)?;
    let mut relationship_types: Vec<RelTypeInfo> = reltype_counts
        .into_iter()
        .map(|(rel_type, count)| RelTypeInfo {
            name: rel_type,
            count,
        })
        .collect();
    relationship_types.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(Json(SchemaResponse {
        labels,
        relationship_types,
    }))
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Count nodes per label by batch-fetching all node data.
fn collect_label_counts(
    store: &impl GraphStore,
    graph: &str,
) -> Result<HashMap<String, usize>, ApiError> {
    let all_ids = store.all_node_ids(graph).map_err(ApiError::from)?;
    let mut counts: HashMap<String, usize> = HashMap::new();

    let batch = store
        .get_nodes_batch(graph, &all_ids)
        .map_err(ApiError::from)?;

    for node in batch.into_iter().flatten() {
        for label in &node.labels {
            *counts.entry(label.clone()).or_insert(0) += 1;
        }
    }
    Ok(counts)
}

/// Count edges per relationship type by batch-fetching all edge data.
fn collect_reltype_counts(
    store: &impl GraphStore,
    graph: &str,
) -> Result<HashMap<String, usize>, ApiError> {
    let all_ids = store.all_edge_ids(graph).map_err(ApiError::from)?;
    let mut counts: HashMap<String, usize> = HashMap::new();

    let batch = store
        .get_edges_batch(graph, &all_ids)
        .map_err(ApiError::from)?;

    for edge in batch.into_iter().flatten() {
        *counts.entry(edge.rel_type.clone()).or_insert(0) += 1;
    }
    Ok(counts)
}
