// gql-redis-store: Redis/Valkey-backed implementation of the GraphStore trait.
//
// Key schema (all keys use {graphname} hash tags for Redis Cluster slot affinity):
//   Node hash:       G:{g}:node:{id}        — HSET labels (JSON), props (JSON)
//   Edge hash:       G:{g}:edge:{id}        — HSET src, dst, rel_type, props (JSON)
//   Node counter:    G:{g}:node_seq         — INCR
//   Edge counter:    G:{g}:edge_seq         — INCR
//   Label index:     G:{g}:label:{label}    — SADD node id (string)
//   RelType index:   G:{g}:reltype:{type}   — SADD edge id (string)
//   Out-adjacency:   G:{g}:out:{node_id}    — SADD "{edge_id}:{neighbor_id}:{rel_type}"
//   In-adjacency:    G:{g}:in:{node_id}     — SADD "{edge_id}:{neighbor_id}:{rel_type}"
//   All nodes set:   G:{g}:nodes            — SADD node id (string)
//   All edges set:   G:{g}:edges            — SADD edge id (string)
//   Index set:       G:{g}:indexes          — SADD "{label}:{property}"
//   Graph registry:  G:{sd2}:graphs         — SADD graph name

pub mod fanout;

pub use fanout::FanoutStore;

use std::collections::HashMap;
use std::env;
use std::sync::Mutex;

use redis::{Client, Commands, Connection};
use serde_json::Value as JsonValue;

use gql_core::error::StorageError;
use gql_core::storage::{
    validate_graph_name, AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor,
    NodeData, NodeId, RangeQuery,
};
use gql_core::types::Value;
use gql_core::types::value::{Crs, GeoPoint};

// ── Value serialisation ───────────────────────────────────────────────────────

/// Serialize a `Value` to a JSON string for storage in Redis.
fn value_to_json(v: &Value) -> Result<JsonValue, StorageError> {
    match v {
        Value::Null => Ok(JsonValue::Null),
        Value::Bool(b) => Ok(JsonValue::Bool(*b)),
        Value::Int(i) => Ok(JsonValue::Number((*i).into())),
        Value::Float(f) => {
            let n = serde_json::Number::from_f64(*f).ok_or_else(|| StorageError::Serialization {
                msg: format!("non-finite float: {f}"),
            })?;
            Ok(JsonValue::Number(n))
        }
        Value::Str(s) => Ok(JsonValue::String(s.to_string())),
        // Temporal and spatial types: serialize as tagged strings.
        Value::Date(d) => Ok(JsonValue::String(format!("__date:{d}"))),
        Value::LocalTime(t) => Ok(JsonValue::String(format!("__localtime:{t}"))),
        Value::LocalDateTime(dt) => Ok(JsonValue::String(format!("__localdatetime:{dt}"))),
        Value::DateTime(dt) => Ok(JsonValue::String(format!("__datetime:{dt}"))),
        Value::Duration(sp) => Ok(JsonValue::String(format!("__duration:{sp}"))),
        Value::Point(p) => Ok(JsonValue::String(format!("__point:{},{},{:?}", p.x, p.y, p.crs))),
        Value::List(items) => {
            let arr: Result<Vec<_>, _> = items.iter().map(value_to_json).collect();
            Ok(JsonValue::Array(arr?))
        }
        Value::Map(m) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in m {
                obj.insert(k.to_string(), value_to_json(v)?);
            }
            Ok(JsonValue::Object(obj))
        }
        // Graph element refs are not stored as property values in Redis hashes.
        Value::Node(id) => Ok(JsonValue::String(format!("__node:{}", id.0))),
        Value::Edge(id) => Ok(JsonValue::String(format!("__edge:{}", id.0))),
        Value::Path { nodes, edges } => {
            let nodes_json: Vec<_> = nodes.iter().map(|n| JsonValue::Number(n.0.into())).collect();
            let edges_json: Vec<_> = edges.iter().map(|e| JsonValue::Number(e.0.into())).collect();
            Ok(JsonValue::Object({
                let mut m = serde_json::Map::new();
                m.insert("__path_nodes".into(), JsonValue::Array(nodes_json));
                m.insert("__path_edges".into(), JsonValue::Array(edges_json));
                m
            }))
        }
    }
}

/// Deserialize a JSON value back to a `Value`.
fn json_to_value(j: &JsonValue) -> Value {
    match j {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(b) => Value::Bool(*b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(f)
            } else {
                Value::Null
            }
        }
        JsonValue::String(s) => {
            // Decode tagged temporal/spatial types.
            if let Some(rest) = s.strip_prefix("__date:") {
                if let Ok(d) = rest.parse::<jiff::civil::Date>() {
                    return Value::Date(d);
                }
            } else if let Some(rest) = s.strip_prefix("__localtime:") {
                if let Ok(t) = rest.parse::<jiff::civil::Time>() {
                    return Value::LocalTime(t);
                }
            } else if let Some(rest) = s.strip_prefix("__localdatetime:") {
                if let Ok(dt) = rest.parse::<jiff::civil::DateTime>() {
                    return Value::LocalDateTime(dt);
                }
            } else if let Some(rest) = s.strip_prefix("__datetime:") {
                if let Ok(dt) = rest.parse::<jiff::Zoned>() {
                    return Value::DateTime(Box::new(dt));
                }
            } else if let Some(rest) = s.strip_prefix("__duration:") {
                if let Ok(sp) = rest.parse::<jiff::Span>() {
                    return Value::Duration(sp);
                }
            } else if let Some(rest) = s.strip_prefix("__point:") {
                // Format: "x,y,CrsVariant" e.g. "1.5,2.5,Wgs84"
                let parts: Vec<&str> = rest.splitn(3, ',').collect();
                if parts.len() == 3 {
                    if let (Ok(x), Ok(y)) = (parts[0].parse::<f64>(), parts[1].parse::<f64>()) {
                        let crs = match parts[2] {
                            "Wgs84" => Crs::Wgs84,
                            "Wgs843d" => Crs::Wgs843d,
                            "Cartesian" => Crs::Cartesian,
                            "Cartesian3d" => Crs::Cartesian3d,
                            _ => Crs::Cartesian,
                        };
                        return Value::Point(GeoPoint { x, y, crs });
                    }
                }
            } else if let Some(rest) = s.strip_prefix("__node:") {
                if let Ok(id) = rest.parse::<u64>() {
                    return Value::Node(NodeId(id));
                }
            } else if let Some(rest) = s.strip_prefix("__edge:") {
                if let Ok(id) = rest.parse::<u64>() {
                    return Value::Edge(EdgeId(id));
                }
            }
            Value::Str(s.as_str().into())
        }
        JsonValue::Array(items) => Value::List(items.iter().map(json_to_value).collect()),
        JsonValue::Object(m) => {
            // Check for encoded path.
            if m.contains_key("__path_nodes") && m.contains_key("__path_edges") {
                let nodes = m["__path_nodes"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_u64().map(NodeId))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let edges = m["__path_edges"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_u64().map(EdgeId))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                return Value::Path { nodes, edges };
            }
            let mut map = indexmap::IndexMap::new();
            for (k, v) in m {
                map.insert(k.as_str().into(), json_to_value(v));
            }
            Value::Map(map)
        }
    }
}

/// Serialize a property map to a JSON string.
fn props_to_json(props: &HashMap<String, Value>) -> Result<String, StorageError> {
    let mut obj = serde_json::Map::new();
    for (k, v) in props {
        obj.insert(k.clone(), value_to_json(v)?);
    }
    serde_json::to_string(&JsonValue::Object(obj)).map_err(|e| StorageError::Serialization {
        msg: e.to_string(),
    })
}

/// Deserialize a JSON string into a property map.
fn json_to_props(s: &str) -> Result<HashMap<String, Value>, StorageError> {
    let j: JsonValue = serde_json::from_str(s).map_err(|e| StorageError::Serialization {
        msg: format!("failed to parse props JSON: {e}"),
    })?;
    match j {
        JsonValue::Object(m) => Ok(m.into_iter().map(|(k, v)| (k, json_to_value(&v))).collect()),
        _ => Err(StorageError::Serialization {
            msg: "props is not a JSON object".into(),
        }),
    }
}

// ── Adjacency encoding ────────────────────────────────────────────────────────

/// Encode an adjacency set member: "{edge_id}:{neighbor_id}:{rel_type}"
fn encode_adj(edge_id: EdgeId, neighbor_id: NodeId, rel_type: &str) -> String {
    format!("{}:{}:{}", edge_id.0, neighbor_id.0, rel_type)
}

/// Decode an adjacency set member. Returns `None` on malformed input.
fn decode_adj(s: &str) -> Option<AdjEntry> {
    // Format: "{edge_id}:{neighbor_id}:{rel_type}"
    // rel_type may contain colons, so split only on the first two colons.
    let mut parts = s.splitn(3, ':');
    let edge_raw = parts.next()?;
    let neighbor_raw = parts.next()?;
    let rel_type = parts.next()?.to_string();
    let edge_id = EdgeId(edge_raw.parse().ok()?);
    let neighbor_id = NodeId(neighbor_raw.parse().ok()?);
    Some(AdjEntry {
        edge_id,
        neighbor_id,
        rel_type,
    })
}

// ── Key helpers ───────────────────────────────────────────────────────────────

fn key_node(g: &str, id: u64) -> String {
    format!("G:{{{g}}}:node:{id}")
}

fn key_edge(g: &str, id: u64) -> String {
    format!("G:{{{g}}}:edge:{id}")
}

fn key_node_seq(g: &str) -> String {
    format!("G:{{{g}}}:node_seq")
}

fn key_edge_seq(g: &str) -> String {
    format!("G:{{{g}}}:edge_seq")
}

fn key_label(g: &str, label: &str) -> String {
    format!("G:{{{g}}}:label:{label}")
}

fn key_reltype(g: &str, rel_type: &str) -> String {
    format!("G:{{{g}}}:reltype:{rel_type}")
}

fn key_out(g: &str, node_id: u64) -> String {
    format!("G:{{{g}}}:out:{node_id}")
}

fn key_in(g: &str, node_id: u64) -> String {
    format!("G:{{{g}}}:in:{node_id}")
}

fn key_nodes_set(g: &str) -> String {
    format!("G:{{{g}}}:nodes")
}

fn key_edges_set(g: &str) -> String {
    format!("G:{{{g}}}:edges")
}

fn key_indexes(g: &str) -> String {
    format!("G:{{{g}}}:indexes")
}

const KEY_GRAPHS: &str = "G:{sd2}:graphs";

// ── RedisGraphStore ───────────────────────────────────────────────────────────

/// A [`GraphStore`] implementation backed by a Redis/Valkey server.
///
/// Connects to a standalone Redis/Valkey instance or Azure Cache for Redis
/// via a URL (`redis://host:port` or `rediss://host:port` for TLS).
///
/// The `REDIS_URL` environment variable is read at construction time if no
/// explicit URL is provided.  Default: `redis://127.0.0.1:6379`.
pub struct RedisGraphStore {
    /// Mutex-protected connection.  All trait methods are synchronous so this
    /// is fine.  A production upgrade could switch to a connection pool.
    conn: Mutex<Connection>,
}

impl RedisGraphStore {
    /// Create a store connecting to the Redis URL given by `GQL_REDIS_URL` or
    /// `REDIS_URL` environment variables, falling back to `redis://127.0.0.1:6379`.
    pub fn from_env() -> Result<Self, StorageError> {
        let url = env::var("GQL_REDIS_URL")
            .or_else(|_| env::var("REDIS_URL"))
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
        Self::from_url(&url)
    }

    /// Create a store connecting to an explicit Redis URL.
    pub fn from_url(url: &str) -> Result<Self, StorageError> {
        let client = Client::open(url).map_err(|e| StorageError::Backend {
            msg: format!("redis client error: {e}"),
        })?;
        let conn = client.get_connection().map_err(|e| StorageError::Backend {
            msg: format!("redis connect error: {e}"),
        })?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Acquire the connection lock.
    fn conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StorageError> {
        self.conn.lock().map_err(|e| StorageError::Backend {
            msg: format!("mutex poisoned: {e}"),
        })
    }

    /// Check that a graph exists; return error if not.
    fn require_graph(&self, g: &str) -> Result<(), StorageError> {
        if !self.graph_exists(g)? {
            return Err(StorageError::GraphNotFound {
                name: g.to_string(),
            });
        }
        Ok(())
    }

    /// Read a node hash from Redis; returns `None` if the key is absent.
    fn read_node(&self, conn: &mut Connection, g: &str, id: u64) -> Result<Option<NodeData>, StorageError> {
        let key = key_node(g, id);
        let fields: Vec<Option<String>> = redis::cmd("HMGET")
            .arg(&key)
            .arg("labels")
            .arg("props")
            .query(conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        match (&fields[0], &fields[1]) {
            (None, _) | (_, None) => Ok(None),
            (Some(labels_json), Some(props_json)) => {
                let labels: Vec<String> =
                    serde_json::from_str(labels_json).map_err(|e| StorageError::Serialization {
                        msg: format!("labels: {e}"),
                    })?;
                let properties = json_to_props(props_json)?;
                Ok(Some(NodeData {
                    id: NodeId(id),
                    labels,
                    properties,
                }))
            }
        }
    }

    /// Read an edge hash from Redis; returns `None` if the key is absent.
    fn read_edge(&self, conn: &mut Connection, g: &str, id: u64) -> Result<Option<EdgeData>, StorageError> {
        let key = key_edge(g, id);
        let fields: Vec<Option<String>> = redis::cmd("HMGET")
            .arg(&key)
            .arg("src")
            .arg("dst")
            .arg("rel_type")
            .arg("props")
            .query(conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        match (&fields[0], &fields[1], &fields[2], &fields[3]) {
            (Some(src_s), Some(dst_s), Some(rel_type), Some(props_json)) => {
                let src = NodeId(
                    src_s
                        .parse::<u64>()
                        .map_err(|_| StorageError::Serialization {
                            msg: "bad src id".into(),
                        })?,
                );
                let dst = NodeId(
                    dst_s
                        .parse::<u64>()
                        .map_err(|_| StorageError::Serialization {
                            msg: "bad dst id".into(),
                        })?,
                );
                let properties = json_to_props(props_json)?;
                Ok(Some(EdgeData {
                    id: EdgeId(id),
                    rel_type: rel_type.clone(),
                    src,
                    dst,
                    properties,
                }))
            }
            _ => Ok(None),
        }
    }

    /// Parse adjacency set members into `AdjEntry` list.
    fn parse_adj_members(members: Vec<String>) -> Vec<AdjEntry> {
        members
            .into_iter()
            .filter_map(|m| decode_adj(&m))
            .collect()
    }

    /// Delete an edge using an already-locked connection. This avoids dropping
    /// and re-acquiring the lock when called from `delete_node(detach=true)`.
    fn delete_edge_inner(
        &self,
        conn: &mut Connection,
        graph: &str,
        id: EdgeId,
    ) -> Result<(), StorageError> {
        let edge = self
            .read_edge(conn, graph, id.0)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        // Remove from rel-type index.
        let _: () = conn.srem(key_reltype(graph, &edge.rel_type), id.0.to_string())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Remove from all-edges set.
        let _: () = conn.srem(key_edges_set(graph), id.0.to_string())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Remove from adjacency sets.
        let out_member = encode_adj(id, edge.dst, &edge.rel_type);
        let in_member = encode_adj(id, edge.src, &edge.rel_type);
        let _: () = conn.srem(key_out(graph, edge.src.0), out_member)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        let _: () = conn.srem(key_in(graph, edge.dst.0), in_member)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Delete edge hash.
        let _: () = conn.del(key_edge(graph, id.0))
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(())
    }
}

// ── GraphStore impl ───────────────────────────────────────────────────────────

impl GraphStore for RedisGraphStore {
    // ── Single-item reads ─────────────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        let mut conn = self.conn()?;
        self.read_node(&mut conn, graph, id.0)
    }

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        let mut conn = self.conn()?;
        self.read_edge(&mut conn, graph, id.0)
    }

    // ── Batch reads ───────────────────────────────────────────────────────────

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let mut conn = self.conn()?;
        let mut results = Vec::with_capacity(ids.len());
        // Pipeline all HMGET commands.
        let mut pipe = redis::pipe();
        for id in ids {
            pipe.cmd("HMGET")
                .arg(key_node(graph, id.0))
                .arg("labels")
                .arg("props");
        }
        let raw: Vec<Vec<Option<String>>> = pipe
            .query(&mut *conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        for (idx, fields) in raw.into_iter().enumerate() {
            let id = ids[idx];
            match (&fields.first().and_then(|f| f.as_ref()), &fields.get(1).and_then(|f| f.as_ref())) {
                (Some(labels_json), Some(props_json)) => {
                    let labels: Vec<String> =
                        serde_json::from_str(labels_json).map_err(|e| StorageError::Serialization {
                            msg: format!("labels batch: {e}"),
                        })?;
                    let properties = json_to_props(props_json)?;
                    results.push(Some(NodeData {
                        id,
                        labels,
                        properties,
                    }));
                }
                _ => results.push(None),
            }
        }
        Ok(results)
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let mut conn = self.conn()?;
        let mut pipe = redis::pipe();
        for id in ids {
            pipe.cmd("HMGET")
                .arg(key_edge(graph, id.0))
                .arg("src")
                .arg("dst")
                .arg("rel_type")
                .arg("props");
        }
        let raw: Vec<Vec<Option<String>>> = pipe
            .query(&mut *conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        let mut results = Vec::with_capacity(ids.len());
        for (idx, fields) in raw.into_iter().enumerate() {
            let id = ids[idx];
            if fields.len() >= 4 {
                match (&fields[0], &fields[1], &fields[2], &fields[3]) {
                    (Some(src_s), Some(dst_s), Some(rel_type), Some(props_json)) => {
                        let src = NodeId(src_s.parse::<u64>().map_err(|_| {
                            StorageError::Serialization { msg: "bad src".into() }
                        })?);
                        let dst = NodeId(dst_s.parse::<u64>().map_err(|_| {
                            StorageError::Serialization { msg: "bad dst".into() }
                        })?);
                        let properties = json_to_props(props_json)?;
                        results.push(Some(EdgeData {
                            id,
                            rel_type: rel_type.clone(),
                            src,
                            dst,
                            properties,
                        }));
                        continue;
                    }
                    _ => {}
                }
            }
            results.push(None);
        }
        Ok(results)
    }

    // ── Label/type index reads ────────────────────────────────────────────────

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        let mut conn = self.conn()?;
        let members: Vec<String> = conn
            .smembers(key_label(graph, label))
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        members
            .into_iter()
            .map(|s| {
                s.parse::<u64>()
                    .map(NodeId)
                    .map_err(|_| StorageError::Serialization {
                        msg: format!("bad node id in label index: {s}"),
                    })
            })
            .collect()
    }

    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        // Fetch all nodes with the label and filter in-memory.
        // A proper index-based approach would require sorted sets; this is
        // correct but not optimised for large label sets.
        let node_ids = self.find_nodes_by_label(graph, label)?;
        let mut conn = self.conn()?;
        let mut result = Vec::new();
        for id in node_ids {
            let node = self.read_node(&mut conn, graph, id.0)?;
            if let Some(node) = node {
                if let Some(val) = node.properties.get(property) {
                    if range_matches(val, &range) {
                        result.push(id);
                    }
                }
            }
        }
        Ok(result)
    }

    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
        let mut conn = self.conn()?;
        let members: Vec<String> = conn
            .smembers(key_reltype(graph, rel_type))
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        members
            .into_iter()
            .map(|s| {
                s.parse::<u64>()
                    .map(EdgeId)
                    .map_err(|_| StorageError::Serialization {
                        msg: format!("bad edge id in reltype index: {s}"),
                    })
            })
            .collect()
    }

    // ── Adjacency reads ───────────────────────────────────────────────────────

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        let mut conn = self.conn()?;
        let members: Vec<String> = conn
            .smembers(key_out(graph, node_id.0))
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(Self::parse_adj_members(members))
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        let mut conn = self.conn()?;
        let members: Vec<String> = conn
            .smembers(key_in(graph, node_id.0))
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(Self::parse_adj_members(members))
    }

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let all = self.out_edges(graph, node_id)?;
        if rel_types.is_empty() {
            return Ok(all);
        }
        Ok(all
            .into_iter()
            .filter(|e| rel_types.contains(&e.rel_type.as_str()))
            .collect())
    }

    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let all = self.in_edges(graph, node_id)?;
        if rel_types.is_empty() {
            return Ok(all);
        }
        Ok(all
            .into_iter()
            .filter(|e| rel_types.contains(&e.rel_type.as_str()))
            .collect())
    }

    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        Ok(self
            .out_edges(graph, node_id)?
            .into_iter()
            .map(|e| e.edge_id)
            .collect())
    }

    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        Ok(self
            .in_edges(graph, node_id)?
            .into_iter()
            .map(|e| e.edge_id)
            .collect())
    }

    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        let mut conn = self.conn()?;
        let members: Vec<String> = conn
            .smembers(key_nodes_set(graph))
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        members
            .into_iter()
            .map(|s| {
                s.parse::<u64>()
                    .map(NodeId)
                    .map_err(|_| StorageError::Serialization {
                        msg: format!("bad node id in nodes set: {s}"),
                    })
            })
            .collect()
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        let mut conn = self.conn()?;
        let members: Vec<String> = conn
            .smembers(key_edges_set(graph))
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        members
            .into_iter()
            .map(|s| {
                s.parse::<u64>()
                    .map(EdgeId)
                    .map_err(|_| StorageError::Serialization {
                        msg: format!("bad edge id in edges set: {s}"),
                    })
            })
            .collect()
    }

    // ── Write operations ──────────────────────────────────────────────────────

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: HashMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        self.require_graph(graph)?;
        let mut conn = self.conn()?;

        // If FanoutStore injected _sd2_id, use it instead of INCR.
        let id = if let Some(Value::Int(i)) = properties.get("_sd2_id") {
            if *i < 0 {
                return Err(StorageError::Backend {
                    msg: format!("_sd2_id must be non-negative, got {i}"),
                });
            }
            *i as u64
        } else {
            let raw_id: i64 = conn
                .incr(key_node_seq(graph), 1)
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            if raw_id <= 0 {
                return Err(StorageError::IdOverflow {
                    graph: graph.to_string(),
                });
            }
            raw_id as u64
        };

        // Strip internal FanoutStore keys before storing.
        let mut clean_props = properties;
        clean_props.remove("_sd2_id");
        clean_props.remove("_sd2_eid");

        let node_key = key_node(graph, id);
        let labels_json =
            serde_json::to_string(&labels).map_err(|e| StorageError::Serialization {
                msg: e.to_string(),
            })?;
        let props_json = props_to_json(&clean_props)?;
        // Store node hash.
        redis::cmd("HSET")
            .arg(&node_key)
            .arg("labels")
            .arg(&labels_json)
            .arg("props")
            .arg(&props_json)
            .query::<()>(&mut *conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Update label indexes.
        for label in &labels {
            let _: () = conn.sadd(key_label(graph, label), id.to_string())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        }
        // Update all-nodes set.
        let _: () = conn.sadd(key_nodes_set(graph), id.to_string())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(NodeId(id))
    }

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: HashMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        self.require_graph(graph)?;
        let mut conn = self.conn()?;
        // Verify src and dst exist.
        if self.read_node(&mut conn, graph, src.0)?.is_none() {
            return Err(StorageError::NodeNotFound { id: src.0 });
        }
        if self.read_node(&mut conn, graph, dst.0)?.is_none() {
            return Err(StorageError::NodeNotFound { id: dst.0 });
        }

        // If FanoutStore injected _sd2_eid, use it instead of INCR.
        let id = if let Some(Value::Int(i)) = properties.get("_sd2_eid") {
            if *i < 0 {
                return Err(StorageError::Backend {
                    msg: format!("_sd2_eid must be non-negative, got {i}"),
                });
            }
            *i as u64
        } else {
            let raw_id: i64 = conn
                .incr(key_edge_seq(graph), 1)
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            if raw_id <= 0 {
                return Err(StorageError::IdOverflow {
                    graph: graph.to_string(),
                });
            }
            raw_id as u64
        };

        // Strip internal FanoutStore keys before storing.
        let mut clean_props = properties;
        clean_props.remove("_sd2_id");
        clean_props.remove("_sd2_eid");

        let edge_key = key_edge(graph, id);
        let props_json = props_to_json(&clean_props)?;
        redis::cmd("HSET")
            .arg(&edge_key)
            .arg("src")
            .arg(src.0.to_string())
            .arg("dst")
            .arg(dst.0.to_string())
            .arg("rel_type")
            .arg(&rel_type)
            .arg("props")
            .arg(&props_json)
            .query::<()>(&mut *conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Update rel-type index.
        let _: () = conn.sadd(key_reltype(graph, &rel_type), id.to_string())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Update adjacency sets.
        let out_member = encode_adj(EdgeId(id), dst, &rel_type);
        let in_member = encode_adj(EdgeId(id), src, &rel_type);
        let _: () = conn.sadd(key_out(graph, src.0), out_member)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        let _: () = conn.sadd(key_in(graph, dst.0), in_member)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Update all-edges set.
        let _: () = conn.sadd(key_edges_set(graph), id.to_string())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(EdgeId(id))
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let node = self
            .read_node(&mut conn, graph, id.0)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        let mut merged = node.properties;
        merged.extend(properties);
        let props_json = props_to_json(&merged)?;
        redis::cmd("HSET")
            .arg(key_node(graph, id.0))
            .arg("props")
            .arg(&props_json)
            .query::<()>(&mut *conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        // Ensure node exists.
        if self.read_node(&mut conn, graph, id.0)?.is_none() {
            return Err(StorageError::NodeNotFound { id: id.0 });
        }
        let props_json = props_to_json(&properties)?;
        redis::cmd("HSET")
            .arg(key_node(graph, id.0))
            .arg("props")
            .arg(&props_json)
            .query::<()>(&mut *conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let edge = self
            .read_edge(&mut conn, graph, id.0)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        let mut merged = edge.properties;
        merged.extend(properties);
        let props_json = props_to_json(&merged)?;
        redis::cmd("HSET")
            .arg(key_edge(graph, id.0))
            .arg("props")
            .arg(&props_json)
            .query::<()>(&mut *conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        if self.read_edge(&mut conn, graph, id.0)?.is_none() {
            return Err(StorageError::EdgeNotFound { id: id.0 });
        }
        let props_json = props_to_json(&properties)?;
        redis::cmd("HSET")
            .arg(key_edge(graph, id.0))
            .arg("props")
            .arg(&props_json)
            .query::<()>(&mut *conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let node = self
            .read_node(&mut conn, graph, id.0)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        let mut props = node.properties;
        props.remove(key);
        let props_json = props_to_json(&props)?;
        redis::cmd("HSET")
            .arg(key_node(graph, id.0))
            .arg("props")
            .arg(&props_json)
            .query::<()>(&mut *conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let edge = self
            .read_edge(&mut conn, graph, id.0)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        let mut props = edge.properties;
        props.remove(key);
        let props_json = props_to_json(&props)?;
        redis::cmd("HSET")
            .arg(key_edge(graph, id.0))
            .arg("props")
            .arg(&props_json)
            .query::<()>(&mut *conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let node = self
            .read_node(&mut conn, graph, id.0)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        let mut existing: std::collections::HashSet<String> =
            node.labels.into_iter().collect();
        for label in labels {
            if existing.insert(label.clone()) {
                // New label: update index.
                let _: () = conn.sadd(key_label(graph, label), id.0.to_string())
                    .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            }
        }
        let new_labels: Vec<String> = existing.into_iter().collect();
        let labels_json =
            serde_json::to_string(&new_labels).map_err(|e| StorageError::Serialization {
                msg: e.to_string(),
            })?;
        redis::cmd("HSET")
            .arg(key_node(graph, id.0))
            .arg("labels")
            .arg(&labels_json)
            .query::<()>(&mut *conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let node = self
            .read_node(&mut conn, graph, id.0)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        let to_remove: std::collections::HashSet<&String> = labels.iter().collect();
        for label in &to_remove {
            let _: () = conn.srem(key_label(graph, label), id.0.to_string())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        }
        let new_labels: Vec<String> = node
            .labels
            .into_iter()
            .filter(|l| !to_remove.contains(l))
            .collect();
        let labels_json =
            serde_json::to_string(&new_labels).map_err(|e| StorageError::Serialization {
                msg: e.to_string(),
            })?;
        redis::cmd("HSET")
            .arg(key_node(graph, id.0))
            .arg("labels")
            .arg(&labels_json)
            .query::<()>(&mut *conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let node = self
            .read_node(&mut conn, graph, id.0)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        // Check for relationships if not detaching.
        if !detach {
            let out: Vec<String> = conn
                .smembers(key_out(graph, id.0))
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            let in_: Vec<String> = conn
                .smembers(key_in(graph, id.0))
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            if !out.is_empty() || !in_.is_empty() {
                return Err(StorageError::Backend {
                    msg: format!(
                        "node {} has relationships; use DETACH DELETE",
                        id.0
                    ),
                });
            }
        }
        if detach {
            // Delete all adjacent edges first, keeping the lock held throughout
            // to prevent new edges from arriving during the operation.
            let out_members: Vec<String> = conn
                .smembers(key_out(graph, id.0))
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            let in_members: Vec<String> = conn
                .smembers(key_in(graph, id.0))
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            let mut edge_ids: Vec<EdgeId> = Vec::new();
            for m in out_members.iter().chain(in_members.iter()) {
                if let Some(adj) = decode_adj(m) {
                    edge_ids.push(adj.edge_id);
                }
            }
            // Deduplicate and delete each edge while holding the lock.
            edge_ids.sort_unstable();
            edge_ids.dedup();
            for eid in edge_ids {
                self.delete_edge_inner(&mut conn, graph, eid)?;
            }
        }
        // Remove label indexes.
        for label in &node.labels {
            let _: () = conn.srem(key_label(graph, label), id.0.to_string())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        }
        // Remove from all-nodes set.
        let _: () = conn.srem(key_nodes_set(graph), id.0.to_string())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Delete adjacency keys.
        let _: () = conn.del(key_out(graph, id.0))
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        let _: () = conn.del(key_in(graph, id.0))
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Delete node hash.
        let _: () = conn.del(key_node(graph, id.0))
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(())
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        self.delete_edge_inner(&mut conn, graph, id)
    }

    // ── Index management ──────────────────────────────────────────────────────

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        let mut conn = self.conn()?;
        let members: Vec<String> = conn
            .smembers(key_indexes(graph))
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        let mut result = Vec::new();
        for m in members {
            if let Some((label, property)) = m.split_once(':') {
                result.push(IndexDescriptor {
                    label: label.to_string(),
                    property: property.to_string(),
                });
            }
        }
        Ok(result)
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.require_graph(graph)?;
        let mut conn = self.conn()?;
        let _: () = conn.sadd(key_indexes(graph), format!("{label}:{property}"))
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(())
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let _: () = conn.srem(key_indexes(graph), format!("{label}:{property}"))
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(())
    }

    // ── Graph administration ──────────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        validate_graph_name(name)?;
        let mut conn = self.conn()?;
        let added: i64 = conn
            .sadd(KEY_GRAPHS, name)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        if added == 0 {
            return Err(StorageError::GraphAlreadyExists {
                name: name.to_string(),
            });
        }
        Ok(())
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        if !self.graph_exists(name)? {
            return Err(StorageError::GraphNotFound {
                name: name.to_string(),
            });
        }
        let mut conn = self.conn()?;
        // Remove from registry.
        let _: () = conn.srem(KEY_GRAPHS, name)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Delete all keys with pattern G:{name}:* using SCAN.
        // We use SCAN with a match pattern to avoid blocking with KEYS.
        let pattern = format!("G:{{{name}}}:*");
        let mut cursor: u64 = 0;
        loop {
            let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(100u64)
                .query(&mut *conn)
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            if !keys.is_empty() {
                let mut del_cmd = redis::cmd("DEL");
                for k in &keys {
                    del_cmd.arg(k);
                }
                del_cmd
                    .query::<()>(&mut *conn)
                    .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            }
            cursor = next_cursor;
            if cursor == 0 {
                break;
            }
        }
        Ok(())
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        let mut conn = self.conn()?;
        conn.sismember(KEY_GRAPHS, name)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        let mut conn = self.conn()?;
        conn.smembers(KEY_GRAPHS)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        self.require_graph(graph)?;
        let mut conn = self.conn()?;
        let node_count: u64 = conn
            .scard(key_nodes_set(graph))
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        let edge_count: u64 = conn
            .scard(key_edges_set(graph))
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Count distinct labels and rel-types by scanning index keys.
        // Pattern: G:{graph}:label:*
        let label_pattern = format!("G:{{{graph}}}:label:*");
        let reltype_pattern = format!("G:{{{graph}}}:reltype:*");
        let label_count = scan_key_count(&mut conn, &label_pattern)?;
        let rel_type_count = scan_key_count(&mut conn, &reltype_pattern)?;
        Ok(GraphStats {
            node_count,
            edge_count,
            label_count: label_count as u32,
            rel_type_count: rel_type_count as u32,
        })
    }
}

/// Count the number of keys matching a pattern via SCAN.
fn scan_key_count(conn: &mut Connection, pattern: &str) -> Result<usize, StorageError> {
    let mut cursor: u64 = 0;
    let mut count = 0usize;
    loop {
        let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(100u64)
            .query(conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        count += keys.len();
        cursor = next;
        if cursor == 0 {
            break;
        }
    }
    Ok(count)
}

// ── Range predicate evaluation ────────────────────────────────────────────────

fn range_matches(val: &Value, range: &RangeQuery<'_>) -> bool {
    match range {
        RangeQuery::Eq(target) => val.cypher_eq(target) == Some(true),
        RangeQuery::Lt(target) => cmp_values(val, target) == Some(std::cmp::Ordering::Less),
        RangeQuery::Lte(target) => {
            matches!(cmp_values(val, target), Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal))
        }
        RangeQuery::Gt(target) => cmp_values(val, target) == Some(std::cmp::Ordering::Greater),
        RangeQuery::Gte(target) => {
            matches!(cmp_values(val, target), Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal))
        }
        RangeQuery::Between { lo, hi, inclusive } => {
            let lo_ok = if *inclusive {
                matches!(cmp_values(val, lo), Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal))
            } else {
                cmp_values(val, lo) == Some(std::cmp::Ordering::Greater)
            };
            let hi_ok = if *inclusive {
                matches!(cmp_values(val, hi), Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal))
            } else {
                cmp_values(val, hi) == Some(std::cmp::Ordering::Less)
            };
            lo_ok && hi_ok
        }
        RangeQuery::StartsWith(prefix) => {
            if let Value::Str(s) = val {
                s.starts_with(prefix)
            } else {
                false
            }
        }
    }
}

fn cmp_values(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Some(x.cmp(y)),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y),
        (Value::Int(x), Value::Float(y)) => (*x as f64).partial_cmp(y),
        (Value::Float(x), Value::Int(y)) => x.partial_cmp(&(*y as f64)),
        (Value::Str(x), Value::Str(y)) => Some(x.cmp(y)),
        _ => None,
    }
}
