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

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value as JValue};
use tracing::{debug, warn};

use gql_core::error::StorageError;
use gql_core::serde_value;
use gql_core::storage::{
    validate_graph_name, AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor,
    NodeData, NodeId, RangeQuery,
};
use gql_core::types::Value;

// ── Property name constants ───────────────────────────────────────────────────
const PROP_GRAPH: &str = "_sd2_graph";
const PROP_NODE_ID: &str = "_sd2_id";
const PROP_EDGE_ID: &str = "_sd2_eid";
const PROP_SRC: &str = "_sd2_src";
const PROP_DST: &str = "_sd2_dst";
const PROP_LABELS: &str = "_sd2_labels"; // JSON-encoded Vec<String> for round-trip fidelity
const SEQ_LABEL: &str = "__SD2Seq";

// ── Token cache ───────────────────────────────────────────────────────────────

struct TokenCache {
    token: String,
    expires_at: Instant,
}

// ── Main struct ───────────────────────────────────────────────────────────────

/// A [`GraphStore`] backed by Microsoft Fabric Graph Database via its REST API.
pub struct FabricGraphStore {
    /// Base URL, e.g. `https://api.fabric.microsoft.com/v1/workspaces/{ws}/graphDatabases/{db}`.
    endpoint: String,
    tenant_id: String,
    client_id: String,
    client_secret: String,
    token: Mutex<Option<TokenCache>>,
    /// Cached HTTP agent with timeout — avoids per-call TLS churn.
    agent: ureq::Agent,
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
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(10))
                .build(),
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

    // ── HTTP agent ─────────────────────────────────────────────────────────────

    /// Return the cached ureq agent with a 10-second timeout for all HTTP calls.
    fn http_agent(&self) -> &ureq::Agent {
        &self.agent
    }

    // ── Token management ──────────────────────────────────────────────────────

    fn bearer_token(&self) -> Result<String, StorageError> {
        let mut cache = self.token.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref c) = *cache {
            // Avoid Instant subtraction panic: check that expiry is far enough in the future.
            if c.expires_at > Instant::now() + Duration::from_secs(60) {
                return Ok(c.token.clone());
            }
        }
        // Fetch new token
        let url = format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
            self.tenant_id
        );
        let body = format!(
            "grant_type=client_credentials&client_id={}&client_secret={}&scope=https%3A%2F%2Fapi.fabric.microsoft.com%2F.default",
            urlenc(&self.client_id),
            urlenc(&self.client_secret),
        );
        let resp: JValue = self
            .http_agent()
            .post(&url)
            .set("Content-Type", "application/x-www-form-urlencoded")
            .send_string(&body)
            .map_err(|e| StorageError::Backend {
                msg: format!("token fetch: {e}"),
            })?
            .into_json()
            .map_err(|e| StorageError::Backend {
                msg: format!("token parse: {e}"),
            })?;

        let token = resp["access_token"]
            .as_str()
            .ok_or_else(|| StorageError::Backend {
                msg: "no access_token in response".into(),
            })?
            .to_string();
        let expires_in = resp["expires_in"].as_u64().unwrap_or(3600);
        *cache = Some(TokenCache {
            token: token.clone(),
            expires_at: Instant::now() + Duration::from_secs(expires_in),
        });
        Ok(token)
    }

    // ── HTTP helpers ──────────────────────────────────────────────────────────

    /// POST a read-only openCypher query.
    fn query(&self, cypher: &str, params: JValue) -> Result<Vec<Vec<JValue>>, StorageError> {
        let token = self.bearer_token()?;
        debug!(cypher, "fabric query");
        let url = format!("{}/query", self.endpoint);
        let body = json!({ "query": cypher, "parameters": params });
        let resp: JValue = self
            .http_agent()
            .post(&url)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Content-Type", "application/json")
            .send_json(body)
            .map_err(|e| StorageError::Backend {
                msg: format!("fabric query: {e}"),
            })?
            .into_json()
            .map_err(|e| StorageError::Backend {
                msg: format!("fabric response parse: {e}"),
            })?;
        parse_rows(&resp)
    }

    /// POST a write openCypher statement.
    fn statement(&self, cypher: &str, params: JValue) -> Result<Vec<Vec<JValue>>, StorageError> {
        let token = self.bearer_token()?;
        debug!(cypher, "fabric statement");
        let url = format!("{}/statements", self.endpoint);
        let body = json!({ "query": cypher, "parameters": params });
        let resp: JValue = self
            .http_agent()
            .post(&url)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Content-Type", "application/json")
            .send_json(body)
            .map_err(|e| StorageError::Backend {
                msg: format!("fabric statement: {e}"),
            })?
            .into_json()
            .map_err(|e| StorageError::Backend {
                msg: format!("fabric response parse: {e}"),
            })?;
        parse_rows(&resp)
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

    // ── Node parsing ──────────────────────────────────────────────────────────

    fn row_to_node_data(&self, row: &[JValue]) -> Result<Option<NodeData>, StorageError> {
        let v = match row.first() {
            Some(v) if !v.is_null() => v,
            _ => return Ok(None),
        };
        let id_raw =
            v["properties"][PROP_NODE_ID]
                .as_u64()
                .ok_or_else(|| StorageError::Serialization {
                    msg: "node missing _sd2_id".into(),
                })?;
        let labels: Vec<String> = v["properties"][PROP_LABELS]
            .as_str()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_else(|| {
                // Fall back to Fabric's native labels, filtering internal ones
                v["labels"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|l| l.as_str())
                            .filter(|l| !l.starts_with("__SD2"))
                            .map(|l| l.to_string())
                            .collect()
                    })
                    .unwrap_or_default()
            });
        let properties = extract_user_props(&v["properties"]);
        Ok(Some(NodeData {
            id: NodeId(id_raw),
            labels,
            properties,
        }))
    }

    fn row_to_edge_data(&self, row: &[JValue]) -> Result<Option<EdgeData>, StorageError> {
        let v = match row.first() {
            Some(v) if !v.is_null() => v,
            _ => return Ok(None),
        };
        let id_raw =
            v["properties"][PROP_EDGE_ID]
                .as_u64()
                .ok_or_else(|| StorageError::Serialization {
                    msg: "edge missing _sd2_eid".into(),
                })?;
        let src =
            v["properties"][PROP_SRC]
                .as_u64()
                .ok_or_else(|| StorageError::Serialization {
                    msg: "edge missing _sd2_src".into(),
                })?;
        let dst =
            v["properties"][PROP_DST]
                .as_u64()
                .ok_or_else(|| StorageError::Serialization {
                    msg: "edge missing _sd2_dst".into(),
                })?;
        let rel_type = v["type"].as_str().unwrap_or("UNKNOWN").to_string();
        let properties = extract_user_props(&v["properties"]);
        Ok(Some(EdgeData {
            id: EdgeId(id_raw),
            rel_type,
            src: NodeId(src),
            dst: NodeId(dst),
            properties,
        }))
    }
}

// ── GraphStore implementation ─────────────────────────────────────────────────

impl GraphStore for FabricGraphStore {
    // ── Reads: single items ───────────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        let rows = self.query(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) RETURN n LIMIT 1"
            ),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        rows.first()
            .map(|r| self.row_to_node_data(r))
            .unwrap_or(Ok(None))
    }

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        let rows = self.query(
            &format!("MATCH ()-[r {{`{PROP_GRAPH}`: $graph, `{PROP_EDGE_ID}`: $id}}]->() RETURN r LIMIT 1"),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        rows.first()
            .map(|r| self.row_to_edge_data(r))
            .unwrap_or(Ok(None))
    }

    // ── Reads: batch ──────────────────────────────────────────────────────────

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let id_list: JValue = ids.iter().map(|n| n.0).collect::<Vec<_>>().into();
        let rows = self.query(
            &format!(
                "UNWIND $ids AS target \
                 OPTIONAL MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: target}}) \
                 RETURN n ORDER BY target"
            ),
            json!({ "graph": graph, "ids": id_list }),
        )?;
        rows.iter().map(|r| self.row_to_node_data(r)).collect()
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let id_list: JValue = ids.iter().map(|e| e.0).collect::<Vec<_>>().into();
        let rows = self.query(
            &format!(
                "UNWIND $ids AS target \
                 OPTIONAL MATCH ()-[r {{`{PROP_GRAPH}`: $graph, `{PROP_EDGE_ID}`: target}}]->() \
                 RETURN r ORDER BY target"
            ),
            json!({ "graph": graph, "ids": id_list }),
        )?;
        rows.iter().map(|r| self.row_to_edge_data(r)).collect()
    }

    // ── Reads: label/type indexes ─────────────────────────────────────────────

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        let label = sanitize_label(label);
        let rows = self.query(
            &format!(
                "MATCH (n:`{label}` {{`{PROP_GRAPH}`: $graph}}) RETURN n.`{PROP_NODE_ID}` AS id"
            ),
            json!({ "graph": graph }),
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_u64().map(NodeId))
            .collect::<Vec<_>>())
    }

    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        let label = sanitize_label(label);
        let prop = sanitize_label(property); // same rules
        let (predicate, params) = range_to_cypher(&prop, range);
        let mut p = params;
        p["graph"] = json!(graph);
        let rows = self.query(
            &format!(
                "MATCH (n:`{label}` {{`{PROP_GRAPH}`: $graph}}) \
                 WHERE {predicate} \
                 RETURN n.`{PROP_NODE_ID}` AS id"
            ),
            p,
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_u64().map(NodeId))
            .collect::<Vec<_>>())
    }

    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
        let rt = sanitize_label(rel_type);
        let rows = self.query(
            &format!(
                "MATCH ()-[r:`{rt}` {{`{PROP_GRAPH}`: $graph}}]->() RETURN r.`{PROP_EDGE_ID}` AS id"
            ),
            json!({ "graph": graph }),
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_u64().map(EdgeId))
            .collect::<Vec<_>>())
    }

    // ── Reads: adjacency ──────────────────────────────────────────────────────

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        let rows = self.query(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}})-[r {{`{PROP_GRAPH}`: $graph}}]->(m) \
                 RETURN r.`{PROP_EDGE_ID}` AS eid, m.`{PROP_NODE_ID}` AS nbr, type(r) AS rt"
            ),
            json!({ "graph": graph, "id": node_id.0 }),
        )?;
        rows_to_adj(rows)
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        let rows = self.query(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}})<-[r {{`{PROP_GRAPH}`: $graph}}]-(m) \
                 RETURN r.`{PROP_EDGE_ID}` AS eid, m.`{PROP_NODE_ID}` AS nbr, type(r) AS rt"
            ),
            json!({ "graph": graph, "id": node_id.0 }),
        )?;
        rows_to_adj(rows)
    }

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let rt_list: JValue = rel_types
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into();
        let rows = self.query(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}})-[r {{`{PROP_GRAPH}`: $graph}}]->(m) \
                 WHERE type(r) IN $types \
                 RETURN r.`{PROP_EDGE_ID}` AS eid, m.`{PROP_NODE_ID}` AS nbr, type(r) AS rt"
            ),
            json!({ "graph": graph, "id": node_id.0, "types": rt_list }),
        )?;
        rows_to_adj(rows)
    }

    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let rt_list: JValue = rel_types
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into();
        let rows = self.query(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}})<-[r {{`{PROP_GRAPH}`: $graph}}]-(m) \
                 WHERE type(r) IN $types \
                 RETURN r.`{PROP_EDGE_ID}` AS eid, m.`{PROP_NODE_ID}` AS nbr, type(r) AS rt"
            ),
            json!({ "graph": graph, "id": node_id.0, "types": rt_list }),
        )?;
        rows_to_adj(rows)
    }

    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        let rows = self.query(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}})-[r {{`{PROP_GRAPH}`: $graph}}]->() \
                 RETURN r.`{PROP_EDGE_ID}` AS id"
            ),
            json!({ "graph": graph, "id": node_id.0 }),
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_u64().map(EdgeId))
            .collect::<Vec<_>>())
    }

    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        let rows = self.query(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}})<-[r {{`{PROP_GRAPH}`: $graph}}]-() \
                 RETURN r.`{PROP_EDGE_ID}` AS id"
            ),
            json!({ "graph": graph, "id": node_id.0 }),
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_u64().map(EdgeId))
            .collect::<Vec<_>>())
    }

    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        let rows = self.query(
            &format!("MATCH (n {{`{PROP_GRAPH}`: $graph}}) WHERE NOT n:{SEQ_LABEL} RETURN n.`{PROP_NODE_ID}` AS id"),
            json!({ "graph": graph }),
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_u64().map(NodeId))
            .collect::<Vec<_>>())
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        let rows = self.query(
            &format!("MATCH ()-[r {{`{PROP_GRAPH}`: $graph}}]->() RETURN r.`{PROP_EDGE_ID}` AS id"),
            json!({ "graph": graph }),
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_u64().map(EdgeId))
            .collect::<Vec<_>>())
    }

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        let rows = self.query(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph}}) WHERE NOT n:{SEQ_LABEL} \
                 RETURN n.`{PROP_LABELS}` AS lbl"
            ),
            json!({ "graph": graph }),
        )?;
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

    // ── Writes: create ────────────────────────────────────────────────────────

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: HashMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        // Use _sd2_id if injected by FanoutStore, otherwise generate.
        let id = if let Some(Value::Int(i)) = properties.get(PROP_NODE_ID) {
            if *i < 0 {
                return Err(StorageError::Backend {
                    msg: format!("_sd2_id must be non-negative, got {i}"),
                });
            }
            NodeId(*i as u64)
        } else {
            self.next_node_id(graph)?
        };

        let label_str = if labels.is_empty() {
            String::new()
        } else {
            format!(
                ":`{}`",
                labels
                    .iter()
                    .map(|l| sanitize_label(l))
                    .collect::<Vec<_>>()
                    .join("`:`")
            )
        };
        let labels_json = serde_json::to_string(&labels).unwrap_or_else(|_| "[]".into());

        // Build property map for the CREATE clause
        let mut all_props: HashMap<String, JValue> = user_props_to_json(&properties);
        all_props.insert(PROP_GRAPH.into(), json!(graph));
        all_props.insert(PROP_NODE_ID.into(), json!(id.0));
        all_props.insert(PROP_LABELS.into(), json!(labels_json));

        self.statement(
            &format!("CREATE (n{label_str} $props) RETURN n.`{PROP_NODE_ID}` AS id"),
            json!({ "props": all_props }),
        )?;

        Ok(id)
    }

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: HashMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        let id = if let Some(Value::Int(i)) = properties.get(PROP_EDGE_ID) {
            if *i < 0 {
                return Err(StorageError::Backend {
                    msg: format!("_sd2_eid must be non-negative, got {i}"),
                });
            }
            EdgeId(*i as u64)
        } else {
            self.next_edge_id(graph)?
        };

        let rt = sanitize_label(&rel_type);
        let mut all_props: HashMap<String, JValue> = user_props_to_json(&properties);
        all_props.insert(PROP_GRAPH.into(), json!(graph));
        all_props.insert(PROP_EDGE_ID.into(), json!(id.0));
        all_props.insert(PROP_SRC.into(), json!(src.0));
        all_props.insert(PROP_DST.into(), json!(dst.0));

        let rows = self.statement(
            &format!(
                "MATCH (a {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $src}}), \
                       (b {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $dst}}) \
                 CREATE (a)-[r:`{rt}` $props]->(b) \
                 RETURN r.`{PROP_EDGE_ID}` AS id"
            ),
            json!({ "graph": graph, "src": src.0, "dst": dst.0, "props": all_props }),
        )?;

        if rows.is_empty() {
            return Err(StorageError::Backend {
                msg: format!(
                    "create_edge: src {src:?} or dst {dst:?} not found in graph '{graph}'"
                ),
            });
        }
        Ok(id)
    }

    // ── Writes: property updates ──────────────────────────────────────────────

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let props = user_props_to_json(&properties);
        self.statement(
            &format!("MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) SET n += $props"),
            json!({ "graph": graph, "id": id.0, "props": props }),
        )?;
        Ok(())
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut props = user_props_to_json(&properties);
        // Preserve internal SD2 properties during replace
        props.insert(PROP_GRAPH.into(), json!(graph));
        props.insert(PROP_NODE_ID.into(), json!(id.0));
        self.statement(
            &format!("MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) SET n = $props"),
            json!({ "graph": graph, "id": id.0, "props": props }),
        )?;
        // Re-sync _sd2_labels from Fabric's native labels (SET n = $props wipes it).
        self.refresh_labels_prop(graph, id)?;
        Ok(())
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let props = user_props_to_json(&properties);
        self.statement(
            &format!("MATCH ()-[r {{`{PROP_GRAPH}`: $graph, `{PROP_EDGE_ID}`: $id}}]->() SET r += $props"),
            json!({ "graph": graph, "id": id.0, "props": props }),
        )?;
        Ok(())
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        // Read current edge to preserve internal src/dst properties
        let edge = self
            .get_edge(graph, id)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        let mut props = user_props_to_json(&properties);
        props.insert(PROP_GRAPH.into(), json!(graph));
        props.insert(PROP_EDGE_ID.into(), json!(id.0));
        props.insert(PROP_SRC.into(), json!(edge.src.0));
        props.insert(PROP_DST.into(), json!(edge.dst.0));
        self.statement(
            &format!(
                "MATCH ()-[r {{`{PROP_GRAPH}`: $graph, `{PROP_EDGE_ID}`: $id}}]->() SET r = $props"
            ),
            json!({ "graph": graph, "id": id.0, "props": props }),
        )?;
        Ok(())
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        let key = sanitize_label(key);
        self.statement(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) REMOVE n.`{key}`"
            ),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        Ok(())
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        let key = sanitize_label(key);
        self.statement(
            &format!("MATCH ()-[r {{`{PROP_GRAPH}`: $graph, `{PROP_EDGE_ID}`: $id}}]->() REMOVE r.`{key}`"),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        Ok(())
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let label_str = labels
            .iter()
            .map(|l| format!("`{}`", sanitize_label(l)))
            .collect::<Vec<_>>()
            .join(":");
        if label_str.is_empty() {
            return Ok(());
        }
        self.statement(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) SET n:{label_str}"
            ),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        // Also update the _sd2_labels property for round-trip fidelity
        // (we'll recompute from the node's current labels)
        self.refresh_labels_prop(graph, id)?;
        Ok(())
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let label_str = labels
            .iter()
            .map(|l| format!("`{}`", sanitize_label(l)))
            .collect::<Vec<_>>()
            .join(":");
        if label_str.is_empty() {
            return Ok(());
        }
        self.statement(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) REMOVE n:{label_str}"
            ),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        self.refresh_labels_prop(graph, id)?;
        Ok(())
    }

    // ── Writes: delete ────────────────────────────────────────────────────────

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        if !detach {
            // Check for any incident relationships first
            let rows = self.query(
                &format!(
                    "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) \
                     OPTIONAL MATCH (n)-[r]-() \
                     RETURN count(r) AS cnt"
                ),
                json!({ "graph": graph, "id": id.0 }),
            )?;
            let cnt = rows
                .first()
                .and_then(|r| r.first())
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if cnt > 0 {
                return Err(StorageError::Backend {
                    msg: format!("node #{} has relationships; use DETACH DELETE", id.0),
                });
            }
        }
        self.statement(
            &format!("MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) DETACH DELETE n"),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        Ok(())
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.statement(
            &format!("MATCH ()-[r {{`{PROP_GRAPH}`: $graph, `{PROP_EDGE_ID}`: $id}}]->() DELETE r"),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        Ok(())
    }

    // ── Index management ──────────────────────────────────────────────────────

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        // Fabric doesn't expose a per-graph index catalog; we store index metadata ourselves.
        let rows = self.query(
            &format!(
                "MATCH (idx:__SD2Index {{`{PROP_GRAPH}`: $graph}}) \
                 RETURN idx._label AS lbl, idx._property AS prop"
            ),
            json!({ "graph": graph }),
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let lbl = r.first()?.as_str()?.to_string();
                let prop = r.get(1)?.as_str()?.to_string();
                Some(IndexDescriptor {
                    label: lbl,
                    property: prop,
                })
            })
            .collect())
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        let label_s = sanitize_label(label);
        let prop_s = sanitize_label(property);
        // Create Fabric native index
        self.statement(
            &format!("CREATE INDEX sd2_{label_s}_{prop_s} IF NOT EXISTS FOR (n:`{label_s}`) ON (n.`{prop_s}`)"),
            json!({}),
        ).unwrap_or_else(|e| { warn!("create_index native: {e}"); vec![] });
        // Record in our metadata store
        self.statement(
            &format!(
                "MERGE (__i:__SD2Index {{`{PROP_GRAPH}`: $graph, _label: $lbl, _property: $prop}})"
            ),
            json!({ "graph": graph, "lbl": label, "prop": property }),
        )?;
        Ok(())
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        let label_s = sanitize_label(label);
        let prop_s = sanitize_label(property);
        self.statement(
            &format!("DROP INDEX sd2_{label_s}_{prop_s} IF EXISTS"),
            json!({}),
        )
        .unwrap_or_else(|e| {
            warn!("drop_index native: {e}");
            vec![]
        });
        self.statement(
            &format!(
                "MATCH (__i:__SD2Index {{`{PROP_GRAPH}`: $graph, _label: $lbl, _property: $prop}}) DELETE __i"
            ),
            json!({ "graph": graph, "lbl": label, "prop": property }),
        )?;
        Ok(())
    }

    // ── Graph administration ──────────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        validate_graph_name(name)?;
        // Idempotently create the sequence counter node that marks the graph's existence.
        self.statement(
            &format!(
                "MERGE (c:{SEQ_LABEL} {{_graph: $graph, _is_root: true}}) \
                 ON CREATE SET c.node_seq = 0, c.edge_seq = 0"
            ),
            json!({ "graph": name }),
        )?;
        Ok(())
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        validate_graph_name(name)?;
        // Delete all nodes (with relationships) and meta nodes for this graph.
        self.statement(
            &format!("MATCH (n {{`{PROP_GRAPH}`: $graph}}) DETACH DELETE n"),
            json!({ "graph": name }),
        )?;
        self.statement(
            &format!("MATCH (c:{SEQ_LABEL} {{_graph: $graph}}) DETACH DELETE c"),
            json!({ "graph": name }),
        )?;
        Ok(())
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        validate_graph_name(name)?;
        let rows = self.query(
            &format!(
                "MATCH (c:{SEQ_LABEL} {{_graph: $graph, _is_root: true}}) RETURN count(c) AS n"
            ),
            json!({ "graph": name }),
        )?;
        Ok(rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            > 0)
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        let rows = self.query(
            &format!("MATCH (c:{SEQ_LABEL} {{_is_root: true}}) RETURN c._graph AS g"),
            json!({}),
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_str().map(|s| s.to_string()))
            .collect())
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        // Use UNWIND to count distinct individual labels, not distinct label-lists.
        // Use OPTIONAL MATCH for edges to handle empty graphs.
        let rows = self.query(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph}}) WHERE NOT n:{SEQ_LABEL} \
                 WITH collect(n) AS nodes, count(n) AS nc \
                 UNWIND CASE WHEN size(nodes) = 0 THEN [null] ELSE nodes END AS n2 \
                 WITH nc, CASE WHEN n2 IS NOT NULL THEN labels(n2) ELSE [] END AS lbls \
                 UNWIND CASE WHEN size(lbls) = 0 THEN [null] ELSE lbls END AS lbl \
                 WITH nc, collect(DISTINCT lbl) AS distinct_labels \
                 WITH nc, size([x IN distinct_labels WHERE x IS NOT NULL AND NOT x STARTS WITH '__SD2']) AS lc \
                 OPTIONAL MATCH ()-[r {{`{PROP_GRAPH}`: $graph}}]->() \
                 WITH nc, lc, count(r) AS ec, count(DISTINCT type(r)) AS rtc \
                 RETURN nc, ec, lc, rtc"
            ),
            json!({ "graph": graph }),
        )?;
        let row = rows.first().cloned().unwrap_or_default();
        Ok(GraphStats {
            node_count: row.first().and_then(|v| v.as_u64()).unwrap_or(0),
            edge_count: row.get(1).and_then(|v| v.as_u64()).unwrap_or(0),
            label_count: row.get(2).and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            rel_type_count: row.get(3).and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        })
    }
}

// ── Private helpers on FabricGraphStore ──────────────────────────────────────

impl FabricGraphStore {
    /// After adding/removing labels via SET/REMOVE, refresh the `_sd2_labels` property
    /// so our metadata stays in sync with Fabric's label storage.
    fn refresh_labels_prop(&self, graph: &str, id: NodeId) -> Result<(), StorageError> {
        let rows = self.query(
            &format!("MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) RETURN labels(n) AS lbls"),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        let labels: Vec<String> = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|l| l.as_str())
                    .filter(|l| !l.starts_with("__SD2"))
                    .map(|l| l.to_string())
                    .collect()
            })
            .unwrap_or_default();
        let labels_json = serde_json::to_string(&labels).unwrap_or_else(|_| "[]".into());
        self.statement(
            &format!("MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) SET n.`{PROP_LABELS}` = $lbls"),
            json!({ "graph": graph, "id": id.0, "lbls": labels_json }),
        )?;
        Ok(())
    }
}

// ── Free helpers ──────────────────────────────────────────────────────────────

/// Parse the Fabric Graph REST API response into `Vec<Vec<JValue>>` (rows × columns).
fn parse_rows(resp: &JValue) -> Result<Vec<Vec<JValue>>, StorageError> {
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

/// Convert adjacency query rows (eid, nbr, rt) to AdjEntry vec.
fn rows_to_adj(rows: Vec<Vec<JValue>>) -> Result<Vec<AdjEntry>, StorageError> {
    rows.into_iter()
        .filter_map(|r| {
            let eid = r.first()?.as_u64().map(EdgeId)?;
            let nbr = r.get(1)?.as_u64().map(NodeId)?;
            let rt = r.get(2)?.as_str()?.to_string();
            Some(Ok(AdjEntry {
                edge_id: eid,
                neighbor_id: nbr,
                rel_type: rt,
            }))
        })
        .collect()
}

/// Extract user-visible properties, stripping all `_sd2_*` internal keys.
fn extract_user_props(props: &JValue) -> HashMap<String, Value> {
    let obj = match props.as_object() {
        Some(o) => o,
        None => return HashMap::new(),
    };
    obj.iter()
        .filter(|(k, _)| !k.starts_with("_sd2") && !k.starts_with("__SD2"))
        .filter_map(|(k, v)| serde_value::json_to_value(v).map(|val| (k.clone(), val)))
        .collect()
}

/// Convert a user property map to JSON, skipping internal SD2 keys.
/// Non-finite floats are silently converted to JSON null.
fn user_props_to_json(props: &HashMap<String, Value>) -> HashMap<String, JValue> {
    props
        .iter()
        .filter(|(k, _)| !k.starts_with("_sd2") && !k.starts_with("__SD2"))
        .map(|(k, v)| {
            (
                k.clone(),
                serde_value::value_to_json(v).unwrap_or(JValue::Null),
            )
        })
        .collect()
}

/// Sanitise a label / property name: keep alphanumeric + underscore, truncate to 128 chars.
fn sanitize_label(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(128)
        .collect()
}

/// URL-encode a string for use in x-www-form-urlencoded bodies.
/// Encodes individual UTF-8 bytes rather than Unicode codepoints, which is the
/// correct behavior per RFC 3986 for multi-byte characters.
fn urlenc(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len() * 3);
    for &byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b => write!(out, "%{b:02X}").unwrap(),
        }
    }
    out
}

/// Build a Cypher WHERE predicate string and parameter map for a RangeQuery.
/// Non-finite floats in range values are serialized as JSON null.
fn range_to_cypher(prop: &str, range: RangeQuery<'_>) -> (String, JValue) {
    let to_json = |v: &Value| serde_value::value_to_json(v).unwrap_or(JValue::Null);
    match range {
        RangeQuery::Eq(v) => (format!("n.`{prop}` = $rv"), json!({ "rv": to_json(v) })),
        RangeQuery::Lt(v) => (format!("n.`{prop}` < $rv"), json!({ "rv": to_json(v) })),
        RangeQuery::Lte(v) => (format!("n.`{prop}` <= $rv"), json!({ "rv": to_json(v) })),
        RangeQuery::Gt(v) => (format!("n.`{prop}` > $rv"), json!({ "rv": to_json(v) })),
        RangeQuery::Gte(v) => (format!("n.`{prop}` >= $rv"), json!({ "rv": to_json(v) })),
        RangeQuery::Between { lo, hi, inclusive } => {
            if inclusive {
                (
                    format!("n.`{prop}` >= $lo AND n.`{prop}` <= $hi"),
                    json!({ "lo": to_json(lo), "hi": to_json(hi) }),
                )
            } else {
                (
                    format!("n.`{prop}` > $lo AND n.`{prop}` < $hi"),
                    json!({ "lo": to_json(lo), "hi": to_json(hi) }),
                )
            }
        }
        RangeQuery::StartsWith(prefix) => (
            format!("n.`{prop}` STARTS WITH $prefix"),
            json!({ "prefix": prefix }),
        ),
    }
}
