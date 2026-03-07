//! Export and import endpoints.
//!
//! GET  /graphs/{name}/export
//! POST /graphs/{name}/import

use std::collections::HashMap;
use std::sync::OnceLock;

use axum::extract::{DefaultBodyLimit, Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};

use gql_core::storage::{validate_graph_name, GraphStore};

use crate::error::ApiError;
use crate::models::{ExportResponse, ImportBody, ImportResponse};
use crate::routes::helpers::json_map_to_props;
use crate::serialize::{edge_to_json, node_to_json};
use crate::state::AppState;

/// Maximum import body size in bytes. Default 2 MiB.
/// Override via `GQL_API_IMPORT_BODY_LIMIT_BYTES`.
fn import_body_limit() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        std::env::var("GQL_API_IMPORT_BODY_LIMIT_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2 * 1024 * 1024)
    })
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/graphs/{name}/export", get(export_handler))
        // /data is the same as /export but without schema — returns just {nodes, edges}
        // Used by the React SPA's graphDataApi.getAll()
        .route("/graphs/{name}/data", get(export_handler))
        .route(
            "/graphs/{name}/import",
            post(import_handler).layer(DefaultBodyLimit::max(import_body_limit())),
        )
}

// ── GET /graphs/{name}/export ─────────────────────────────────────────────────

async fn export_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<ExportResponse>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;

    let store = state
        .get_store(&name)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("graph '{name}' not found")))?;

    // Export all nodes.
    let all_node_ids = store.all_node_ids(&name).map_err(ApiError::from)?;
    let node_batch = store
        .get_nodes_batch(&name, &all_node_ids)
        .map_err(ApiError::from)?;
    let nodes: Vec<serde_json::Value> = node_batch.iter().flatten().map(node_to_json).collect();

    // Export all edges.
    let all_edge_ids = store.all_edge_ids(&name).map_err(ApiError::from)?;
    let edge_batch = store
        .get_edges_batch(&name, &all_edge_ids)
        .map_err(ApiError::from)?;
    let edges: Vec<serde_json::Value> = edge_batch.iter().flatten().map(edge_to_json).collect();

    // Build schema summary inline from the already-fetched batches to avoid
    // a redundant second scan of all nodes and edges.
    let mut label_counts: HashMap<String, usize> = HashMap::new();
    for node in node_batch.iter().flatten() {
        for label in &node.labels {
            *label_counts.entry(label.clone()).or_insert(0) += 1;
        }
    }
    let mut reltype_counts: HashMap<String, usize> = HashMap::new();
    for edge in edge_batch.iter().flatten() {
        *reltype_counts.entry(edge.rel_type.clone()).or_insert(0) += 1;
    }

    let schema_labels: Vec<serde_json::Value> = {
        let mut v: Vec<(&String, &usize)> = label_counts.iter().collect();
        v.sort_by_key(|(k, _)| k.as_str());
        v.into_iter()
            .map(|(k, c)| serde_json::json!({ "name": k, "count": c }))
            .collect()
    };
    let schema_reltypes: Vec<serde_json::Value> = {
        let mut v: Vec<(&String, &usize)> = reltype_counts.iter().collect();
        v.sort_by_key(|(k, _)| k.as_str());
        v.into_iter()
            .map(|(k, c)| serde_json::json!({ "name": k, "count": c }))
            .collect()
    };

    let schema = serde_json::json!({
        "labels": schema_labels,
        "relationship_types": schema_reltypes,
    });

    Ok(Json(ExportResponse {
        nodes,
        edges,
        schema,
    }))
}

// ── POST /graphs/{name}/import ────────────────────────────────────────────────
//
// An explicit 2 MB body size limit is applied via DefaultBodyLimit on the route.

async fn import_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<ImportBody>,
) -> Result<Json<ImportResponse>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;

    // Guard: reject imports that exceed the element limit to prevent DoS via
    // unbounded memory allocation.  The limit is configurable via env var.
    let import_limit: usize = std::env::var("GQL_API_IMPORT_LIMIT")
        .ok()
        .and_then(|v| match v.parse() {
            Ok(val) => Some(val),
            Err(_) => {
                tracing::warn!(
                    "invalid value for GQL_API_IMPORT_LIMIT: {v:?}, using default 10000"
                );
                None
            }
        })
        .unwrap_or(10_000);
    let total_elements = body.nodes.len().saturating_add(body.edges.len());
    if total_elements > import_limit {
        return Err(ApiError::bad_request(format!(
            "import payload too large: {total_elements} elements exceeds the \
             {import_limit} element limit (nodes + edges)"
        )));
    }

    let store = state
        .get_store(&name)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("graph '{name}' not found")))?;

    // Map from import ID string to newly-created NodeId, so edges can reference them.
    let mut id_map: HashMap<String, gql_core::storage::NodeId> = HashMap::new();
    let mut nodes_created: usize = 0;

    for import_node in body.nodes {
        let props = json_map_to_props(import_node.properties)?;
        let new_id = store
            .create_node(&name, import_node.labels, props)
            .map_err(ApiError::from)?;
        nodes_created += 1;

        // If the import payload supplied a stable string ID, record the mapping.
        if let Some(ref stable_id) = import_node.id {
            id_map.insert(stable_id.clone(), new_id);
        }
        // Also register by the numeric value so that imports from export can
        // resolve edges by numeric string IDs.
        id_map.insert(new_id.0.to_string(), new_id);
    }

    // Attempt to create each edge; collect failures rather than aborting early.
    // InMemoryGraphStore does not support transactions, so partial writes cannot
    // be rolled back — we report what was actually created.
    let mut edges_created: usize = 0;
    let mut edge_warnings: Vec<String> = Vec::new();

    for import_edge in body.edges {
        // Validate relationship type before touching the store.
        if import_edge.rel_type.is_empty() {
            edge_warnings.push(format!(
                "skipped edge {}->{}: relationship type must not be empty",
                import_edge.source, import_edge.target
            ));
            continue;
        }

        // Resolve source/target: first try the id_map, then parse as raw u64.
        let src = match resolve_import_node_id(&import_edge.source, &id_map) {
            Ok(id) => id,
            Err(_) => {
                edge_warnings.push(format!(
                    "skipped edge {}->{}: cannot resolve source node reference '{}'",
                    import_edge.source, import_edge.target, import_edge.source
                ));
                continue;
            }
        };
        let dst = match resolve_import_node_id(&import_edge.target, &id_map) {
            Ok(id) => id,
            Err(_) => {
                edge_warnings.push(format!(
                    "skipped edge {}->{}: cannot resolve target node reference '{}'",
                    import_edge.source, import_edge.target, import_edge.target
                ));
                continue;
            }
        };

        let props = match json_map_to_props(import_edge.properties) {
            Ok(p) => p,
            Err(_) => {
                edge_warnings.push(format!(
                    "skipped edge {}->{}: invalid property value",
                    import_edge.source, import_edge.target
                ));
                continue;
            }
        };

        match store.create_edge(&name, src, dst, import_edge.rel_type, props) {
            Ok(_) => edges_created += 1,
            Err(e) => {
                edge_warnings.push(format!(
                    "skipped edge {}->{}: {}",
                    import_edge.source, import_edge.target, e
                ));
            }
        }
    }

    if !edge_warnings.is_empty() {
        tracing::warn!(
            graph = %name,
            nodes_created,
            edges_created,
            warnings = ?edge_warnings,
            "import completed with partial edge failures"
        );
    }

    Ok(Json(ImportResponse {
        nodes_created,
        edges_created,
        warnings: edge_warnings,
    }))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn resolve_import_node_id(
    s: &str,
    id_map: &HashMap<String, gql_core::storage::NodeId>,
) -> Result<gql_core::storage::NodeId, ApiError> {
    if let Some(&id) = id_map.get(s) {
        return Ok(id);
    }
    s.parse::<u64>()
        .map(gql_core::storage::NodeId)
        .map_err(|_| {
            ApiError::bad_request(format!(
                "cannot resolve node reference '{s}': not in id map and not a valid u64"
            ))
        })
}
