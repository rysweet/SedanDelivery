// HTTP helpers: query / statement execution against the Fabric Graph REST API.

use gql_core::error::StorageError;
use serde_json::{json, Value as JValue};
use tracing::debug;

use crate::FabricGraphStore;

impl FabricGraphStore {
    /// Build the correct query URL based on the endpoint type.
    /// GraphModel endpoints use /executeQuery?preview=true (GQL Query API).
    /// Legacy endpoints use /query (read) and /statements (write).
    pub(crate) fn query_url(&self, is_write: bool) -> String {
        if self.endpoint.contains("/GraphModels/") {
            // Fabric Graph GQL Query API — single endpoint for all operations
            format!("{}/executeQuery?preview=true", self.endpoint)
        } else if is_write {
            format!("{}/statements", self.endpoint)
        } else {
            format!("{}/query", self.endpoint)
        }
    }

    /// POST a read-only GQL/openCypher query.
    pub(crate) fn query(
        &self,
        cypher: &str,
        params: JValue,
    ) -> Result<Vec<Vec<JValue>>, StorageError> {
        let token = self.bearer_token()?;
        debug!(cypher, "fabric query");
        let url = self.query_url(false);
        let body = json!({ "query": cypher, "parameters": params });
        let resp: JValue = self
            .agent
            .post(&url)
            .header("Authorization", &format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .send_json(body)
            .map_err(|e| StorageError::Backend {
                msg: format!("fabric query: {e}"),
            })?
            .body_mut()
            .read_json()
            .map_err(|e| StorageError::Backend {
                msg: format!("fabric response parse: {e}"),
            })?;
        parse_rows(&resp)
    }

    /// POST a write GQL/openCypher statement.
    pub(crate) fn statement(
        &self,
        cypher: &str,
        params: JValue,
    ) -> Result<Vec<Vec<JValue>>, StorageError> {
        let token = self.bearer_token()?;
        debug!(cypher, "fabric statement");
        let url = self.query_url(true);
        let body = json!({ "query": cypher, "parameters": params });
        let resp: JValue = self
            .agent
            .post(&url)
            .header("Authorization", &format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .send_json(body)
            .map_err(|e| StorageError::Backend {
                msg: format!("fabric statement: {e}"),
            })?
            .body_mut()
            .read_json()
            .map_err(|e| StorageError::Backend {
                msg: format!("fabric response parse: {e}"),
            })?;
        parse_rows(&resp)
    }
}

/// Parse the Fabric Graph REST API response into `Vec<Vec<JValue>>` (rows x columns).
pub(crate) fn parse_rows(resp: &JValue) -> Result<Vec<Vec<JValue>>, StorageError> {
    // Fabric response format:
    // { "results": [ { "headers": [...], "data": [[col0, col1, ...], ...] } ] }
    if let Some(err) = resp.get("error").or_else(|| resp.get("message")) {
        return Err(StorageError::Backend {
            msg: format!("Fabric API error: {err}"),
        });
    }
    let results = resp
        .get("results")
        .and_then(|r| r.as_array())
        .ok_or_else(|| StorageError::Serialization {
            msg: format!("unexpected Fabric response: {resp}"),
        })?;

    let mut all_rows: Vec<Vec<JValue>> = Vec::new();
    for result in results {
        // Check for per-result embedded errors before reading data.
        if let Some(err) = result.get("error") {
            return Err(StorageError::Backend {
                msg: format!("Fabric result error: {err}"),
            });
        }
        if let Some(data) = result.get("data").and_then(|d| d.as_array()) {
            for row in data {
                let cells: Vec<JValue> = row.as_array().cloned().unwrap_or_default();
                all_rows.push(cells);
            }
        }
    }
    Ok(all_rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FabricGraphStore;
    use serde_json::json;

    fn make_store(endpoint: &str) -> FabricGraphStore {
        FabricGraphStore::new(
            endpoint.to_string(),
            "tenant".into(),
            "client".into(),
            "secret".into(),
        )
    }

    // ── query_url ────────────────────────────────────────────────────────────

    #[test]
    fn query_url_graph_model_endpoint() {
        let store =
            make_store("https://api.fabric.microsoft.com/v1/workspaces/ws1/GraphModels/gm1");
        assert_eq!(
            store.query_url(false),
            "https://api.fabric.microsoft.com/v1/workspaces/ws1/GraphModels/gm1/executeQuery?preview=true"
        );
        assert_eq!(
            store.query_url(true),
            "https://api.fabric.microsoft.com/v1/workspaces/ws1/GraphModels/gm1/executeQuery?preview=true"
        );
    }

    #[test]
    fn query_url_legacy_endpoint_read() {
        let store =
            make_store("https://api.fabric.microsoft.com/v1/workspaces/ws1/graphDatabases/db1");
        assert_eq!(
            store.query_url(false),
            "https://api.fabric.microsoft.com/v1/workspaces/ws1/graphDatabases/db1/query"
        );
    }

    #[test]
    fn query_url_legacy_endpoint_write() {
        let store =
            make_store("https://api.fabric.microsoft.com/v1/workspaces/ws1/graphDatabases/db1");
        assert_eq!(
            store.query_url(true),
            "https://api.fabric.microsoft.com/v1/workspaces/ws1/graphDatabases/db1/statements"
        );
    }

    #[test]
    fn query_url_trailing_slash_stripped() {
        let store =
            make_store("https://api.fabric.microsoft.com/v1/workspaces/ws1/graphDatabases/db1/");
        assert!(store.query_url(false).ends_with("/query"));
    }

    // ── parse_rows ──────────────────────────────────────────────────────────

    #[test]
    fn parse_rows_valid_response() {
        let resp = json!({
            "results": [{
                "headers": ["id", "name"],
                "data": [[1, "Alice"], [2, "Bob"]]
            }]
        });
        let rows = parse_rows(&resp).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec![json!(1), json!("Alice")]);
        assert_eq!(rows[1], vec![json!(2), json!("Bob")]);
    }

    #[test]
    fn parse_rows_multiple_results() {
        let resp = json!({
            "results": [
                { "data": [[1]] },
                { "data": [[2], [3]] }
            ]
        });
        let rows = parse_rows(&resp).unwrap();
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn parse_rows_empty_results() {
        let resp = json!({ "results": [] });
        let rows = parse_rows(&resp).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn parse_rows_empty_data() {
        let resp = json!({ "results": [{ "data": [] }] });
        let rows = parse_rows(&resp).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn parse_rows_result_without_data() {
        let resp = json!({ "results": [{ "headers": ["x"] }] });
        let rows = parse_rows(&resp).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn parse_rows_top_level_error() {
        let resp = json!({ "error": "Unauthorized" });
        let err = parse_rows(&resp).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Fabric API error"), "got: {msg}");
    }

    #[test]
    fn parse_rows_top_level_message_error() {
        let resp = json!({ "message": "Rate limited" });
        let err = parse_rows(&resp).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Fabric API error"), "got: {msg}");
    }

    #[test]
    fn parse_rows_per_result_error() {
        let resp = json!({ "results": [{ "error": "syntax error at line 1" }] });
        let err = parse_rows(&resp).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Fabric result error"), "got: {msg}");
    }

    #[test]
    fn parse_rows_missing_results_key() {
        let resp = json!({ "data": "wrong shape" });
        let err = parse_rows(&resp).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("unexpected Fabric response"), "got: {msg}");
    }

    #[test]
    fn parse_rows_non_array_row_becomes_empty_vec() {
        let resp = json!({ "results": [{ "data": ["not_an_array", [1, 2]] }] });
        let rows = parse_rows(&resp).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].is_empty());
        assert_eq!(rows[1], vec![json!(1), json!(2)]);
    }
}
