//! Schema introspection endpoint.
//!
//! GET /graphs/{name}/schema
//!
//! Returns label counts and relationship-type counts derived from a full
//! node / edge scan via the storage layer.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};

use gql_core::storage::validate_graph_name;

use crate::error::ApiError;
use crate::models::{LabelInfo, RelTypeInfo, SchemaResponse};
use crate::routes::helpers::{compute_schema_counts_blocking, require_graph};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/graphs/{name}/schema", get(schema_handler))
}

async fn schema_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<SchemaResponse>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    require_graph(&state, &name).await?;

    let (label_counts, reltype_counts) = compute_schema_counts_blocking(&state, &name).await?;

    let mut labels: Vec<LabelInfo> = label_counts
        .into_iter()
        .map(|(label, count)| LabelInfo { name: label, count })
        .collect();
    labels.sort_by(|a, b| a.name.cmp(&b.name));

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
