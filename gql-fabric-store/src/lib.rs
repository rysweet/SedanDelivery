// gql-fabric-store: Microsoft Fabric Graph Database-backed GraphStore.
//
// Architecture
// ─────────────
// Microsoft Fabric exposes a Graph Database REST API that speaks openCypher.
// We implement the GraphStore trait by translating every method into one or
// more openCypher queries POSTed to the Fabric endpoint.
//
// All graph-namespacing is done via the `_sd2_graph` property stored on every
// node and relationship, so a single Fabric database can hold multiple named
// graphs.  The canonical integer IDs (u64) that our trait uses are stored in
// the `_sd2_id` property.
//
// Configuration (environment variables)
// ──────────────────────────────────────
//   GQL_FABRIC_ENDPOINT   https://api.fabric.microsoft.com/v1/workspaces/{ws}/graphDatabases/{db}
//   GQL_FABRIC_TENANT_ID  Azure AD tenant ID for token acquisition
//   GQL_FABRIC_CLIENT_ID  Service-principal app ID
//   GQL_FABRIC_CLIENT_SECRET  Service-principal client secret
//
// ID generation (standalone mode)
// ─────────────────────────────────
// When used as the primary store, node/edge IDs are assigned via a Fabric-
// stored counter node (__SD2Seq).  When used as a FanoutStore secondary, the
// primary (Redis) already assigned the ID; FanoutStore injects it as a
// `_sd2_id` entry in the properties map before calling create_node/create_edge
// on secondaries, so we just use it directly.

mod auth;
mod http;
mod parsing;
mod sanitize;
mod store;

use std::sync::Mutex;

use serde_json::json;

use gql_core::error::StorageError;
use gql_core::storage::{EdgeId, NodeId};

use auth::TokenCache;

// ── Property name constants ───────────────────────────────────────────────────
pub(crate) const PROP_GRAPH: &str = "_sd2_graph";
pub(crate) const PROP_NODE_ID: &str = "_sd2_id";
pub(crate) const PROP_EDGE_ID: &str = "_sd2_eid";
pub(crate) const PROP_SRC: &str = "_sd2_src";
pub(crate) const PROP_DST: &str = "_sd2_dst";
pub(crate) const PROP_LABELS: &str = "_sd2_labels"; // JSON-encoded Vec<String> for round-trip fidelity
pub(crate) const SEQ_LABEL: &str = "__SD2Seq";

// ── Main struct ───────────────────────────────────────────────────────────────

/// A [`GraphStore`](gql_core::storage::GraphStore) backed by Microsoft Fabric Graph Database via its REST API.
pub struct FabricGraphStore {
    /// Base URL, e.g. `https://api.fabric.microsoft.com/v1/workspaces/{ws}/graphDatabases/{db}`.
    pub(crate) endpoint: String,
    pub(crate) tenant_id: String,
    pub(crate) client_id: String,
    pub(crate) client_secret: String,
    pub(crate) token: Mutex<Option<TokenCache>>,
    /// Cached HTTP agent with timeout — avoids per-call TLS churn.
    pub(crate) agent: ureq::Agent,
}

impl FabricGraphStore {
    /// Construct from explicit credentials.
    pub fn new(
        endpoint: String,
        tenant_id: String,
        client_id: String,
        client_secret: String,
    ) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            tenant_id,
            client_id,
            client_secret,
            token: Mutex::new(None),
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(std::time::Duration::from_secs(
                    std::env::var("GQL_FABRIC_TIMEOUT_SECS")
                        .ok()
                        .and_then(|v| match v.parse() {
                            Ok(val) => Some(val),
                            Err(_) => {
                                tracing::warn!(
                                    "invalid value for GQL_FABRIC_TIMEOUT_SECS: {v:?}, using default 10"
                                );
                                None
                            }
                        })
                        .unwrap_or(10),
                )))
                .build()
                .new_agent(),
        }
    }

    /// Construct from environment variables (GQL_FABRIC_*).
    pub fn from_env() -> Result<Self, StorageError> {
        let get = |k: &str| {
            std::env::var(k).map_err(|_| StorageError::Backend {
                msg: format!("missing env var {k}"),
            })
        };
        Ok(Self::new(
            get("GQL_FABRIC_ENDPOINT")?,
            get("GQL_FABRIC_TENANT_ID")?,
            get("GQL_FABRIC_CLIENT_ID")?,
            get("GQL_FABRIC_CLIENT_SECRET")?,
        ))
    }

    // ── ID generation ─────────────────────────────────────────────────────────

    fn next_node_id(&self, graph: &str) -> Result<NodeId, StorageError> {
        let rows = self.statement(
            &format!(
                "MERGE (c:{SEQ_LABEL} {{_graph: $graph}}) \
                 ON CREATE SET c.node_seq = 0 \
                 SET c.node_seq = coalesce(c.node_seq, 0) + 1 \
                 RETURN c.node_seq AS id"
            ),
            json!({ "graph": graph }),
        )?;
        let raw = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.as_u64())
            .ok_or_else(|| StorageError::Backend {
                msg: "no node_seq returned".into(),
            })?;
        Ok(NodeId(raw))
    }

    fn next_edge_id(&self, graph: &str) -> Result<EdgeId, StorageError> {
        let rows = self.statement(
            &format!(
                "MERGE (c:{SEQ_LABEL} {{_graph: $graph}}) \
                 ON CREATE SET c.edge_seq = 0 \
                 SET c.edge_seq = coalesce(c.edge_seq, 0) + 1 \
                 RETURN c.edge_seq AS id"
            ),
            json!({ "graph": graph }),
        )?;
        let raw = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.as_u64())
            .ok_or_else(|| StorageError::Backend {
                msg: "no edge_seq returned".into(),
            })?;
        Ok(EdgeId(raw))
    }
}
