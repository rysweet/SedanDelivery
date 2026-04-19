//! Schema-count computation: label and relationship-type counts for a graph.

use std::collections::HashMap;

use cypher_core::storage::GraphStore;

use crate::error::ApiError;
use crate::state::AppState;

use super::store::run_store_blocking;

/// Label and relationship-type count maps returned by [`compute_schema_counts`].
pub(crate) type SchemaCounts = (HashMap<String, usize>, HashMap<String, usize>);

/// Maximum number of elements (nodes + edges) that `compute_schema_counts` will
/// load into memory. Defaults to 1 000 000. Override via `GQL_API_MAX_SCHEMA_SCAN_ELEMENTS`.
///
/// If a graph exceeds this limit, schema-count requests are rejected with 413
/// rather than risking an out-of-memory condition.
fn max_schema_scan_elements() -> u64 {
    use std::sync::OnceLock;
    static LIMIT: OnceLock<u64> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        std::env::var("GQL_API_MAX_SCHEMA_SCAN_ELEMENTS")
            .ok()
            .and_then(|v| match v.parse() {
                Ok(val) if val > 0 => Some(val),
                _ => {
                    tracing::warn!(
                        "invalid value for GQL_API_MAX_SCHEMA_SCAN_ELEMENTS, using default 1000000"
                    );
                    None
                }
            })
            .unwrap_or(1_000_000)
    })
}

/// Compute label counts and relationship-type counts for the given graph.
///
/// Returns `(label_counts, reltype_counts)` where each map associates a name
/// with the number of nodes/edges carrying that label/type.
pub(crate) fn compute_schema_counts(
    store: &dyn GraphStore,
    graph: &str,
) -> Result<SchemaCounts, ApiError> {
    // Guard: use graph_stats() (count-only, no data fetched) to reject
    // graphs that are too large before allocating memory for all nodes/edges.
    let stats = store.graph_stats(graph).map_err(ApiError::from)?;
    let total_elements = stats.node_count.saturating_add(stats.edge_count);
    let limit = max_schema_scan_elements();
    if total_elements > limit {
        return Err(ApiError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "graph '{graph}' has {total_elements} elements, which exceeds the \
                 schema scan limit of {limit}. \
                 Set GQL_API_MAX_SCHEMA_SCAN_ELEMENTS to raise the limit."
            ),
        ));
    }

    // Label counts from all nodes.
    let all_node_ids = store.all_node_ids(graph).map_err(ApiError::from)?;
    let node_batch = store
        .get_nodes_batch(graph, &all_node_ids)
        .map_err(ApiError::from)?;
    let mut label_counts: HashMap<String, usize> = HashMap::new();
    for node in node_batch.into_iter().flatten() {
        for label in &node.labels {
            *label_counts.entry(label.clone()).or_insert(0) += 1;
        }
    }

    // Relationship-type counts from all edges.
    let all_edge_ids = store.all_edge_ids(graph).map_err(ApiError::from)?;
    let edge_batch = store
        .get_edges_batch(graph, &all_edge_ids)
        .map_err(ApiError::from)?;
    let mut reltype_counts: HashMap<String, usize> = HashMap::new();
    for edge in edge_batch.into_iter().flatten() {
        *reltype_counts.entry(edge.rel_type.clone()).or_insert(0) += 1;
    }

    Ok((label_counts, reltype_counts))
}

pub(crate) async fn compute_schema_counts_blocking(
    state: &AppState,
    graph: &str,
) -> Result<SchemaCounts, ApiError> {
    let graph = graph.to_string();
    run_store_blocking(state, move |store| {
        compute_schema_counts(store.as_ref(), &graph)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use cypher_core::storage::InMemoryGraphStore;
    use indexmap::IndexMap;

    #[test]
    fn compute_schema_counts_empty_graph() {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();

        let (labels, rels) = compute_schema_counts(&store, "g").unwrap();
        assert!(labels.is_empty());
        assert!(rels.is_empty());
    }

    #[test]
    fn compute_schema_counts_with_nodes_and_edges() {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();

        let n1 = store
            .create_node("g", vec!["Person".to_string()], IndexMap::new())
            .unwrap();
        let n2 = store
            .create_node(
                "g",
                vec!["Person".to_string(), "Employee".to_string()],
                IndexMap::new(),
            )
            .unwrap();
        let n3 = store
            .create_node("g", vec!["Company".to_string()], IndexMap::new())
            .unwrap();

        store
            .create_edge("g", n1, n2, "KNOWS".to_string(), IndexMap::new())
            .unwrap();
        store
            .create_edge("g", n2, n3, "WORKS_AT".to_string(), IndexMap::new())
            .unwrap();
        store
            .create_edge("g", n1, n3, "KNOWS".to_string(), IndexMap::new())
            .unwrap();

        let (labels, rels) = compute_schema_counts(&store, "g").unwrap();

        assert_eq!(labels.get("Person"), Some(&2));
        assert_eq!(labels.get("Employee"), Some(&1));
        assert_eq!(labels.get("Company"), Some(&1));

        assert_eq!(rels.get("KNOWS"), Some(&2));
        assert_eq!(rels.get("WORKS_AT"), Some(&1));
    }

    #[test]
    fn compute_schema_counts_nonexistent_graph() {
        let store = InMemoryGraphStore::new();
        let result = compute_schema_counts(&store, "nonexistent");
        assert!(result.is_err());
    }
}
