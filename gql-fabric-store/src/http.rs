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
