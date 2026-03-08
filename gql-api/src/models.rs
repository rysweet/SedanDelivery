//! Request and response body models for all API endpoints.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

// ── Health ────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub demo_graph_exists: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ── Graphs ────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateGraphBody {
    pub name: String,
}

#[derive(Serialize)]
pub struct GraphListResponse {
    pub graphs: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphStatsResponse {
    pub node_count: u64,
    pub edge_count: u64,
    pub label_count: u32,
    pub rel_type_count: u32,
}

// ── Nodes ─────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateNodeBody {
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub properties: serde_json::Map<String, Json>,
}

#[derive(Deserialize)]
pub struct UpdateNodeBody {
    #[serde(default)]
    pub properties: serde_json::Map<String, Json>,
}

#[derive(Serialize)]
pub struct NodeListResponse {
    pub nodes: Vec<Json>,
    pub total: usize,
}

// ── Edges ─────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateEdgeBody {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub rel_type: String,
    #[serde(default)]
    pub properties: serde_json::Map<String, Json>,
}

#[derive(Deserialize)]
pub struct UpdateEdgeBody {
    #[serde(default)]
    pub properties: serde_json::Map<String, Json>,
}

#[derive(Serialize)]
pub struct EdgeListResponse {
    pub edges: Vec<Json>,
    pub total: usize,
}

// ── Query / Mutate ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CypherBody {
    pub cypher: String,
    #[serde(default)]
    pub params: Option<serde_json::Map<String, Json>>,
}

#[derive(Serialize)]
pub struct QueryResponse {
    pub columns: Vec<String>,
    pub rows: Vec<Json>,
    pub elapsed_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutateResponse {
    pub nodes_created: u64,
    pub nodes_deleted: u64,
    pub edges_created: u64,
    pub edges_deleted: u64,
    pub properties_set: u64,
}

// ── Explain ──────────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplainResponse {
    pub plan: serde_json::Value,
}

// ── Pagination query params ───────────────────────────────────────────────────

/// Return the maximum number of items that may be requested in a single page.
///
/// Reads `GQL_API_MAX_PAGE_LIMIT` from the environment on first call (default `10_000`).
pub fn max_page_limit() -> usize {
    use std::sync::OnceLock;
    static VAL: OnceLock<usize> = OnceLock::new();
    *VAL.get_or_init(|| {
        std::env::var("GQL_API_MAX_PAGE_LIMIT")
            .ok()
            .and_then(|v| match v.parse() {
                Ok(val) => Some(val),
                Err(_) => {
                    tracing::warn!(
                        "invalid value for GQL_API_MAX_PAGE_LIMIT: {v:?}, using default 10000"
                    );
                    None
                }
            })
            .unwrap_or(10_000)
    })
}

/// Return the default page size when the caller does not supply a `limit` parameter.
///
/// Reads `GQL_API_DEFAULT_PAGE_LIMIT` from the environment on first call (default `100`).
pub fn default_page_limit() -> usize {
    use std::sync::OnceLock;
    static VAL: OnceLock<usize> = OnceLock::new();
    *VAL.get_or_init(|| {
        std::env::var("GQL_API_DEFAULT_PAGE_LIMIT")
            .ok()
            .and_then(|v| match v.parse() {
                Ok(val) => Some(val),
                Err(_) => {
                    tracing::warn!(
                        "invalid value for GQL_API_DEFAULT_PAGE_LIMIT: {v:?}, using default 100"
                    );
                    None
                }
            })
            .unwrap_or(100)
    })
}

#[derive(Deserialize)]
pub struct PaginationParams {
    #[serde(default = "default_page_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

// ── Schema ────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct LabelInfo {
    pub name: String,
    pub count: usize,
}

#[derive(Serialize)]
pub struct RelTypeInfo {
    pub name: String,
    pub count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaResponse {
    pub labels: Vec<LabelInfo>,
    pub relationship_types: Vec<RelTypeInfo>,
}

// ── Neighbors ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct NeighborParams {
    /// "in", "out", or "both" (default: "both")
    pub direction: Option<String>,
    /// Comma-separated relationship types to filter
    pub types: Option<String>,
    #[serde(default = "default_neighbor_limit")]
    pub limit: usize,
}

/// Return the default neighbor limit when the caller does not supply a `limit` parameter.
///
/// Reads `GQL_API_DEFAULT_NEIGHBOR_LIMIT` from the environment on first call (default `50`).
fn default_neighbor_limit() -> usize {
    use std::sync::OnceLock;
    static VAL: OnceLock<usize> = OnceLock::new();
    *VAL.get_or_init(|| {
        std::env::var("GQL_API_DEFAULT_NEIGHBOR_LIMIT")
            .ok()
            .and_then(|v| match v.parse() {
                Ok(val) => Some(val),
                Err(_) => {
                    tracing::warn!(
                        "invalid value for GQL_API_DEFAULT_NEIGHBOR_LIMIT: {v:?}, using default 50"
                    );
                    None
                }
            })
            .unwrap_or(50)
    })
}

#[derive(Serialize)]
pub struct NeighborResponse {
    pub nodes: Vec<Json>,
    pub edges: Vec<Json>,
}

// ── Export / Import ───────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ExportResponse {
    pub nodes: Vec<Json>,
    pub edges: Vec<Json>,
    pub schema: Json,
}

#[derive(Deserialize)]
pub struct ImportBody {
    #[serde(default)]
    pub nodes: Vec<ImportNode>,
    #[serde(default)]
    pub edges: Vec<ImportEdge>,
}

#[derive(Deserialize)]
pub struct ImportNode {
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub properties: serde_json::Map<String, Json>,
    /// Optional stable ID used by ImportEdge to refer to this node.
    pub id: Option<String>,
}

#[derive(Deserialize)]
pub struct ImportEdge {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub rel_type: String,
    #[serde(default)]
    pub properties: serde_json::Map<String, Json>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResponse {
    pub nodes_created: usize,
    pub edges_created: usize,
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── HealthResponse serialization ─────────────────────────────────────────

    #[test]
    fn health_response_serializes_camel_case() {
        let resp = HealthResponse {
            status: "ok",
            version: "0.1.0",
            demo_graph_exists: Some(true),
            error: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["version"], "0.1.0");
        assert_eq!(json["demoGraphExists"], true);
        assert!(json.get("demo_graph_exists").is_none());
    }

    // ── GraphStatsResponse serialization ─────────────────────────────────────

    #[test]
    fn graph_stats_response_serializes_camel_case() {
        let resp = GraphStatsResponse {
            node_count: 100,
            edge_count: 200,
            label_count: 3,
            rel_type_count: 5,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["nodeCount"], 100);
        assert_eq!(json["edgeCount"], 200);
        assert_eq!(json["labelCount"], 3);
        assert_eq!(json["relTypeCount"], 5);
    }

    // ── CreateGraphBody deserialization ───────────────────────────────────────

    #[test]
    fn create_graph_body_deserializes() {
        let body: CreateGraphBody = serde_json::from_value(json!({"name": "social"})).unwrap();
        assert_eq!(body.name, "social");
    }

    #[test]
    fn create_graph_body_missing_name_fails() {
        let result = serde_json::from_value::<CreateGraphBody>(json!({}));
        assert!(result.is_err());
    }

    // ── CreateNodeBody deserialization ────────────────────────────────────────

    #[test]
    fn create_node_body_all_fields() {
        let body: CreateNodeBody = serde_json::from_value(json!({
            "labels": ["Person", "Employee"],
            "properties": {"name": "Alice", "age": 30}
        }))
        .unwrap();
        assert_eq!(body.labels, vec!["Person", "Employee"]);
        assert_eq!(body.properties.len(), 2);
    }

    #[test]
    fn create_node_body_defaults_empty() {
        let body: CreateNodeBody = serde_json::from_value(json!({})).unwrap();
        assert!(body.labels.is_empty());
        assert!(body.properties.is_empty());
    }

    // ── CreateEdgeBody deserialization ────────────────────────────────────────

    #[test]
    fn create_edge_body_deserializes() {
        let body: CreateEdgeBody = serde_json::from_value(json!({
            "source": "1",
            "target": "2",
            "type": "KNOWS",
            "properties": {"since": 2020}
        }))
        .unwrap();
        assert_eq!(body.source, "1");
        assert_eq!(body.target, "2");
        assert_eq!(body.rel_type, "KNOWS");
        assert_eq!(body.properties.len(), 1);
    }

    #[test]
    fn create_edge_body_missing_required_fields_fails() {
        let result = serde_json::from_value::<CreateEdgeBody>(json!({"source": "1"}));
        assert!(result.is_err());
    }

    // ── CypherBody deserialization ───────────────────────────────────────────

    #[test]
    fn cypher_body_deserializes() {
        let body: CypherBody =
            serde_json::from_value(json!({"cypher": "MATCH (n) RETURN n"})).unwrap();
        assert_eq!(body.cypher, "MATCH (n) RETURN n");
        assert!(body.params.is_none());
    }

    #[test]
    fn cypher_body_with_params() {
        let body: CypherBody = serde_json::from_value(json!({
            "cypher": "MATCH (n) WHERE n.name = $name RETURN n",
            "params": {"name": "Alice", "age": 30}
        }))
        .unwrap();
        assert_eq!(body.cypher, "MATCH (n) WHERE n.name = $name RETURN n");
        let params = body.params.unwrap();
        assert_eq!(params.len(), 2);
        assert_eq!(params["name"], json!("Alice"));
        assert_eq!(params["age"], json!(30));
    }

    #[test]
    fn cypher_body_without_params_defaults_to_none() {
        let body: CypherBody = serde_json::from_value(json!({"cypher": "RETURN 1"})).unwrap();
        assert!(body.params.is_none());
    }

    #[test]
    fn cypher_body_with_empty_params() {
        let body: CypherBody =
            serde_json::from_value(json!({"cypher": "RETURN 1", "params": {}})).unwrap();
        let params = body.params.unwrap();
        assert!(params.is_empty());
    }

    // ── QueryResponse serialization ──────────────────────────────────────────

    #[test]
    fn query_response_serializes() {
        let resp = QueryResponse {
            columns: vec!["n".to_string()],
            rows: vec![json!({"n": 1})],
            elapsed_ms: 42,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["columns"], json!(["n"]));
        assert_eq!(json["rows"].as_array().unwrap().len(), 1);
        assert_eq!(json["elapsed_ms"], 42);
    }

    // ── MutateResponse serialization ─────────────────────────────────────────

    #[test]
    fn mutate_response_serializes_camel_case() {
        let resp = MutateResponse {
            nodes_created: 1,
            nodes_deleted: 0,
            edges_created: 2,
            edges_deleted: 0,
            properties_set: 5,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["nodesCreated"], 1);
        assert_eq!(json["nodesDeleted"], 0);
        assert_eq!(json["edgesCreated"], 2);
        assert_eq!(json["edgesDeleted"], 0);
        assert_eq!(json["propertiesSet"], 5);
    }

    // ── PaginationParams deserialization ─────────────────────────────────────

    #[test]
    fn pagination_params_with_values() {
        let params: PaginationParams =
            serde_json::from_value(json!({"limit": 50, "offset": 10})).unwrap();
        assert_eq!(params.limit, 50);
        assert_eq!(params.offset, 10);
    }

    #[test]
    fn pagination_params_offset_defaults_to_zero() {
        let params: PaginationParams = serde_json::from_value(json!({"limit": 50})).unwrap();
        assert_eq!(params.limit, 50);
        assert_eq!(params.offset, 0);
    }

    // ── NeighborParams deserialization ────────────────────────────────────────

    #[test]
    fn neighbor_params_defaults() {
        let params: NeighborParams = serde_json::from_value(json!({})).unwrap();
        assert!(params.direction.is_none());
        assert!(params.types.is_none());
        assert_eq!(params.limit, 50);
    }

    #[test]
    fn neighbor_params_with_values() {
        let params: NeighborParams = serde_json::from_value(json!({
            "direction": "out",
            "types": "KNOWS,FOLLOWS",
            "limit": 10
        }))
        .unwrap();
        assert_eq!(params.direction.as_deref(), Some("out"));
        assert_eq!(params.types.as_deref(), Some("KNOWS,FOLLOWS"));
        assert_eq!(params.limit, 10);
    }

    // ── SchemaResponse serialization ─────────────────────────────────────────

    #[test]
    fn schema_response_serializes_camel_case() {
        let resp = SchemaResponse {
            labels: vec![LabelInfo {
                name: "Person".to_string(),
                count: 10,
            }],
            relationship_types: vec![RelTypeInfo {
                name: "KNOWS".to_string(),
                count: 5,
            }],
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["labels"][0]["name"], "Person");
        assert_eq!(json["labels"][0]["count"], 10);
        assert_eq!(json["relationshipTypes"][0]["name"], "KNOWS");
    }

    // ── ImportBody deserialization ────────────────────────────────────────────

    #[test]
    fn import_body_deserializes() {
        let body: ImportBody = serde_json::from_value(json!({
            "nodes": [
                {"labels": ["Person"], "properties": {"name": "Alice"}, "id": "n1"}
            ],
            "edges": [
                {"source": "n1", "target": "n2", "type": "KNOWS"}
            ]
        }))
        .unwrap();
        assert_eq!(body.nodes.len(), 1);
        assert_eq!(body.nodes[0].labels, vec!["Person"]);
        assert_eq!(body.nodes[0].id, Some("n1".to_string()));
        assert_eq!(body.edges.len(), 1);
        assert_eq!(body.edges[0].rel_type, "KNOWS");
    }

    #[test]
    fn import_body_defaults_empty() {
        let body: ImportBody = serde_json::from_value(json!({})).unwrap();
        assert!(body.nodes.is_empty());
        assert!(body.edges.is_empty());
    }

    // ── ImportResponse serialization ─────────────────────────────────────────

    #[test]
    fn import_response_serializes_camel_case() {
        let resp = ImportResponse {
            nodes_created: 3,
            edges_created: 2,
            warnings: vec!["edge ref not found".to_string()],
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["nodesCreated"], 3);
        assert_eq!(json["edgesCreated"], 2);
        assert_eq!(json["warnings"][0], "edge ref not found");
    }

    // ── GraphListResponse serialization ──────────────────────────────────────

    #[test]
    fn graph_list_response_serializes() {
        let resp = GraphListResponse {
            graphs: vec!["social".to_string(), "test".to_string()],
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["graphs"], json!(["social", "test"]));
    }

    // ── NodeListResponse and EdgeListResponse serialization ──────────────────

    #[test]
    fn node_list_response_serializes() {
        let resp = NodeListResponse {
            nodes: vec![json!({"id": "1"})],
            total: 1,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["total"], 1);
        assert_eq!(json["nodes"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn edge_list_response_serializes() {
        let resp = EdgeListResponse {
            edges: vec![json!({"id": "1"})],
            total: 1,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["total"], 1);
        assert_eq!(json["edges"].as_array().unwrap().len(), 1);
    }

    // ── ExportResponse serialization ─────────────────────────────────────────

    #[test]
    fn export_response_serializes() {
        let resp = ExportResponse {
            nodes: vec![json!({"id": "1"})],
            edges: vec![],
            schema: json!({"labels": []}),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["nodes"].as_array().unwrap().len(), 1);
        assert!(json["edges"].as_array().unwrap().is_empty());
    }
}
