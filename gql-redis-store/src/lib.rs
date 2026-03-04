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
//   Graph registry:  __sd2_registry:graphs   — SADD graph name

pub mod fanout;

pub use fanout::FanoutStore;

use std::collections::HashMap;
use std::env;
use std::time::Duration;

use redis::cluster::ClusterClient;
use redis::{Client, ConnectionLike};

use gql_core::error::StorageError;
use gql_core::serde_value::{json_to_props, props_to_json};
use gql_core::storage::{
    validate_graph_name, AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor,
    NodeData, NodeId, RangeQuery,
};
use gql_core::types::Value;

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

/// Property index key: maps (label, property, value) → SET of node IDs.
/// Enables O(1) MERGE lookups instead of O(N) full-label scans.
fn key_propidx(g: &str, label: &str, property: &str, value_key: &str) -> String {
    format!("G:{{{g}}}:propidx:{label}:{property}:{value_key}")
}

/// Convert a Value to a canonical string for use as a Redis key component.
/// Must be deterministic: same value always produces same string.
fn value_to_index_key(val: &Value) -> String {
    match val {
        Value::Int(i) => format!("i:{i}"),
        Value::Float(f) => format!("f:{}", f.to_bits()), // bitwise for determinism
        Value::Str(s) => format!("s:{s}"),
        Value::Bool(b) => format!("b:{b}"),
        // For complex types, use Debug representation.
        other => format!("j:{other:?}"),
    }
}

/// Registry key for graph names.  Uses a `__sd2_registry:` prefix that cannot
/// collide with per-graph data keys (which all start with `G:`).  The key has
/// no Redis Cluster hash tag, so it routes to its own slot; this is fine because
/// it's only accessed by standalone operations (SADD, SISMEMBER, SMEMBERS, SREM).
const KEY_GRAPHS: &str = "__sd2_registry:graphs";

// ── RedisGraphStore ───────────────────────────────────────────────────────────

/// Default connection pool size. Set high enough for concurrent bulk writes
/// (each Bolt session may hold a pool connection during query execution).
const DEFAULT_POOL_SIZE: u32 = 64;
/// Default pool checkout timeout in seconds.
const DEFAULT_POOL_TIMEOUT_SECS: u64 = 5;

/// Connection pool supporting both standalone and Redis Cluster modes.
///
/// Standalone mode uses a single Redis URL; cluster mode uses multiple seed
/// node URLs and auto-discovers the full cluster topology.
enum RedisPool {
    Standalone(r2d2::Pool<Client>),
    Cluster(r2d2::Pool<ClusterClient>),
}

/// A checked-out connection from either pool type.
///
/// Both `Connection` and `ClusterConnection` implement `redis::ConnectionLike`,
/// so single-command operations work identically via [`RedisConn::as_mut`].
enum RedisConn {
    Standalone(r2d2::PooledConnection<Client>),
    Cluster(Box<r2d2::PooledConnection<ClusterClient>>),
}

impl RedisConn {
    /// Get a mutable reference to the underlying connection as `ConnectionLike`.
    ///
    /// Both `Connection` and `ClusterConnection` implement `ConnectionLike`,
    /// so all `redis::cmd()` based operations work through this reference.
    /// Note: the `redis::Commands` trait requires `Sized` so cannot be used
    /// on `dyn ConnectionLike`; use `redis::cmd()` directly instead.
    fn as_mut(&mut self) -> &mut dyn ConnectionLike {
        match self {
            RedisConn::Standalone(c) => &mut **c,
            RedisConn::Cluster(c) => &mut **c,
        }
    }
}

/// A [`GraphStore`] implementation backed by a Redis/Valkey server or cluster.
///
/// Connects to a standalone Redis/Valkey instance, Azure Cache for Redis,
/// or a Redis Cluster via multiple seed-node URLs.
///
/// Uses an `r2d2` connection pool for concurrent access from multiple threads
/// (e.g. multiple Bolt sessions sharing a single `RedisGraphStore`).
///
/// Connection configuration:
/// - `GQL_REDIS_CLUSTER_NODES` — comma-separated seed node URLs for cluster mode
/// - `GQL_REDIS_URL` / `REDIS_URL` — standalone URL (fallback `redis://127.0.0.1:6379`)
///
/// Pool configuration:
/// - `GQL_REDIS_POOL_SIZE` — max connections (default 16)
/// - `GQL_REDIS_POOL_TIMEOUT_SECS` — checkout timeout (default 5)
pub struct RedisGraphStore {
    pool: RedisPool,
    /// Optional key prefix for multi-tenant isolation.
    /// Set via `GQL_REDIS_KEY_PREFIX` environment variable (default empty).
    key_prefix: String,
}

impl RedisGraphStore {
    /// Create a store from environment variables.
    ///
    /// Cluster mode is selected when `GQL_REDIS_CLUSTER_NODES` is set (comma-separated
    /// seed-node URLs).  Otherwise falls back to standalone mode using `GQL_REDIS_URL`,
    /// `REDIS_URL`, or `redis://127.0.0.1:6379`.
    ///
    /// The key prefix is read from `GQL_REDIS_KEY_PREFIX` (default empty string).
    /// Set a prefix to isolate multiple deployments sharing a single Redis instance.
    ///
    /// Pool size and timeout are read from `GQL_REDIS_POOL_SIZE` (default 16)
    /// and `GQL_REDIS_POOL_TIMEOUT_SECS` (default 5).
    pub fn from_env() -> Result<Self, StorageError> {
        let prefix = env::var("GQL_REDIS_KEY_PREFIX").unwrap_or_default();
        let pool_size: u32 = env::var("GQL_REDIS_POOL_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_POOL_SIZE);
        let pool_timeout_secs: u64 = env::var("GQL_REDIS_POOL_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_POOL_TIMEOUT_SECS);

        // Cluster mode: GQL_REDIS_CLUSTER_NODES takes priority.
        if let Ok(nodes_str) = env::var("GQL_REDIS_CLUSTER_NODES") {
            let nodes: Vec<&str> = nodes_str.split(',').map(|s| s.trim()).collect();
            return Self::from_cluster_nodes(&nodes, &prefix, pool_size, pool_timeout_secs);
        }

        // Standalone mode.
        let url = env::var("GQL_REDIS_URL")
            .or_else(|_| env::var("REDIS_URL"))
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
        Self::from_url_with_options(&url, &prefix, pool_size, pool_timeout_secs)
    }

    /// Create a store connecting to an explicit Redis URL with an optional key prefix.
    pub fn from_url_with_prefix(url: &str, key_prefix: &str) -> Result<Self, StorageError> {
        Self::from_url_with_options(
            url,
            key_prefix,
            DEFAULT_POOL_SIZE,
            DEFAULT_POOL_TIMEOUT_SECS,
        )
    }

    /// Create a store with full configuration control (standalone mode).
    pub fn from_url_with_options(
        url: &str,
        key_prefix: &str,
        pool_size: u32,
        pool_timeout_secs: u64,
    ) -> Result<Self, StorageError> {
        let client = Client::open(url).map_err(|e| StorageError::Backend {
            msg: format!("redis client error: {e}"),
        })?;
        let pool = r2d2::Pool::builder()
            .max_size(pool_size)
            .connection_timeout(Duration::from_secs(pool_timeout_secs))
            .build(client)
            .map_err(|e| StorageError::Backend {
                msg: format!("redis pool error: {e}"),
            })?;
        Ok(Self {
            pool: RedisPool::Standalone(pool),
            key_prefix: key_prefix.to_string(),
        })
    }

    /// Create a store connecting to a Redis Cluster via seed node URLs.
    ///
    /// The cluster client auto-discovers the full topology from the seed nodes.
    /// All keys for a given graph route to the same hash slot via `{graphname}`
    /// hash tags in the key schema.
    pub fn from_cluster_nodes(
        nodes: &[&str],
        key_prefix: &str,
        pool_size: u32,
        pool_timeout_secs: u64,
    ) -> Result<Self, StorageError> {
        let client = ClusterClient::new(nodes.iter().map(|s| s.to_string()).collect::<Vec<_>>())
            .map_err(|e| StorageError::Backend {
                msg: format!("redis cluster client error: {e}"),
            })?;
        let pool = r2d2::Pool::builder()
            .max_size(pool_size)
            .connection_timeout(Duration::from_secs(pool_timeout_secs))
            .build(client)
            .map_err(|e| StorageError::Backend {
                msg: format!("redis cluster pool error: {e}"),
            })?;
        Ok(Self {
            pool: RedisPool::Cluster(pool),
            key_prefix: key_prefix.to_string(),
        })
    }

    /// Create a store connecting to an explicit Redis URL with no key prefix.
    pub fn from_url(url: &str) -> Result<Self, StorageError> {
        Self::from_url_with_prefix(url, "")
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Check out a connection from the pool.
    fn conn(&self) -> Result<RedisConn, StorageError> {
        match &self.pool {
            RedisPool::Standalone(pool) => {
                pool.get()
                    .map(RedisConn::Standalone)
                    .map_err(|e| StorageError::Backend {
                        msg: format!("redis pool checkout error: {e}"),
                    })
            }
            RedisPool::Cluster(pool) => pool
                .get()
                .map(|c| RedisConn::Cluster(Box::new(c)))
                .map_err(|e| StorageError::Backend {
                    msg: format!("redis cluster pool checkout error: {e}"),
                }),
        }
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

    /// Apply the configured key prefix to `key`.
    ///
    /// When `key_prefix` is empty (the default) this is a no-op clone.
    #[inline]
    fn kp(&self, key: String) -> String {
        if self.key_prefix.is_empty() {
            key
        } else {
            format!("{}{}", self.key_prefix, key)
        }
    }

    /// Return the prefixed graph-registry key.
    #[inline]
    fn key_graphs(&self) -> String {
        self.kp(KEY_GRAPHS.to_string())
    }

    /// Read a node hash from Redis; returns `None` if the key is absent.
    fn read_node(
        &self,
        conn: &mut dyn ConnectionLike,
        g: &str,
        id: u64,
    ) -> Result<Option<NodeData>, StorageError> {
        let key = self.kp(key_node(g, id));
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
    fn read_edge(
        &self,
        conn: &mut dyn ConnectionLike,
        g: &str,
        id: u64,
    ) -> Result<Option<EdgeData>, StorageError> {
        let key = self.kp(key_edge(g, id));
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
                let src =
                    NodeId(
                        src_s
                            .parse::<u64>()
                            .map_err(|_| StorageError::Serialization {
                                msg: "bad src id".into(),
                            })?,
                    );
                let dst =
                    NodeId(
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
        members.into_iter().filter_map(|m| decode_adj(&m)).collect()
    }

    /// Check which indexes exist for a given graph (returns set of "label:property" strings).
    fn active_indexes(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
    ) -> Result<std::collections::HashSet<String>, StorageError> {
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_indexes(graph)))
            .query(conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(members.into_iter().collect())
    }

    /// Add a node's properties to all active property indexes.
    fn index_node_properties(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        node_id: u64,
        labels: &[String],
        properties: &HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let indexes = self.active_indexes(conn, graph)?;
        if indexes.is_empty() {
            return Ok(());
        }
        let mut pipe = redis::pipe();
        let mut has_cmds = false;
        for idx_spec in &indexes {
            if let Some((idx_label, idx_prop)) = idx_spec.split_once(':') {
                if labels.contains(&idx_label.to_string()) {
                    if let Some(val) = properties.get(idx_prop) {
                        if !val.is_null() {
                            let vkey = value_to_index_key(val);
                            pipe.cmd("SADD")
                                .arg(self.kp(key_propidx(graph, idx_label, idx_prop, &vkey)))
                                .arg(node_id.to_string())
                                .ignore();
                            has_cmds = true;
                        }
                    }
                }
            }
        }
        if has_cmds {
            pipe.query::<()>(conn)
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        }
        Ok(())
    }

    /// Remove a node's properties from all active property indexes.
    fn deindex_node_properties(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        node_id: u64,
        labels: &[String],
        properties: &HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let indexes = self.active_indexes(conn, graph)?;
        if indexes.is_empty() {
            return Ok(());
        }
        let mut pipe = redis::pipe();
        let mut has_cmds = false;
        for idx_spec in &indexes {
            if let Some((idx_label, idx_prop)) = idx_spec.split_once(':') {
                if labels.contains(&idx_label.to_string()) {
                    if let Some(val) = properties.get(idx_prop) {
                        if !val.is_null() {
                            let vkey = value_to_index_key(val);
                            pipe.cmd("SREM")
                                .arg(self.kp(key_propidx(graph, idx_label, idx_prop, &vkey)))
                                .arg(node_id.to_string())
                                .ignore();
                            has_cmds = true;
                        }
                    }
                }
            }
        }
        if has_cmds {
            pipe.query::<()>(conn)
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        }
        Ok(())
    }

    /// Delete an edge using an already-locked connection. This avoids dropping
    /// and re-acquiring the lock when called from `delete_node(detach=true)`.
    fn delete_edge_inner(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        id: EdgeId,
    ) -> Result<(), StorageError> {
        let edge = self
            .read_edge(conn, graph, id.0)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        // Remove from rel-type index.
        redis::cmd("SREM")
            .arg(self.kp(key_reltype(graph, &edge.rel_type)))
            .arg(id.0.to_string())
            .query::<()>(conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Remove from all-edges set.
        redis::cmd("SREM")
            .arg(self.kp(key_edges_set(graph)))
            .arg(id.0.to_string())
            .query::<()>(conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Remove from adjacency sets.
        let out_member = encode_adj(id, edge.dst, &edge.rel_type);
        let in_member = encode_adj(id, edge.src, &edge.rel_type);
        redis::cmd("SREM")
            .arg(self.kp(key_out(graph, edge.src.0)))
            .arg(out_member)
            .query::<()>(conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        redis::cmd("SREM")
            .arg(self.kp(key_in(graph, edge.dst.0)))
            .arg(in_member)
            .query::<()>(conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Delete edge hash.
        redis::cmd("DEL")
            .arg(self.kp(key_edge(graph, id.0)))
            .query::<()>(conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(())
    }
}

// ── GraphStore impl ───────────────────────────────────────────────────────────

impl GraphStore for RedisGraphStore {
    // ── Single-item reads ─────────────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        let mut conn = self.conn()?;
        self.read_node(conn.as_mut(), graph, id.0)
    }

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        let mut conn = self.conn()?;
        self.read_edge(conn.as_mut(), graph, id.0)
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
                .arg(self.kp(key_node(graph, id.0)))
                .arg("labels")
                .arg("props");
        }
        let raw: Vec<Vec<Option<String>>> = pipe
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        for (idx, fields) in raw.into_iter().enumerate() {
            let id = ids[idx];
            match (
                &fields.first().and_then(|f| f.as_ref()),
                &fields.get(1).and_then(|f| f.as_ref()),
            ) {
                (Some(labels_json), Some(props_json)) => {
                    let labels: Vec<String> = serde_json::from_str(labels_json).map_err(|e| {
                        StorageError::Serialization {
                            msg: format!("labels batch: {e}"),
                        }
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
                .arg(self.kp(key_edge(graph, id.0)))
                .arg("src")
                .arg("dst")
                .arg("rel_type")
                .arg("props");
        }
        let raw: Vec<Vec<Option<String>>> = pipe
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        let mut results = Vec::with_capacity(ids.len());
        for (idx, fields) in raw.into_iter().enumerate() {
            let id = ids[idx];
            if fields.len() >= 4 {
                if let (Some(src_s), Some(dst_s), Some(rel_type), Some(props_json)) =
                    (&fields[0], &fields[1], &fields[2], &fields[3])
                {
                    let src =
                        NodeId(
                            src_s
                                .parse::<u64>()
                                .map_err(|_| StorageError::Serialization {
                                    msg: "bad src".into(),
                                })?,
                        );
                    let dst =
                        NodeId(
                            dst_s
                                .parse::<u64>()
                                .map_err(|_| StorageError::Serialization {
                                    msg: "bad dst".into(),
                                })?,
                        );
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
            }
            results.push(None);
        }
        Ok(results)
    }

    // ── Label/type index reads ────────────────────────────────────────────────

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        let mut conn = self.conn()?;
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_label(graph, label)))
            .query(conn.as_mut())
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
        let mut conn = self.conn()?;

        // Fast path: use property index for exact-match queries when an index exists.
        if let RangeQuery::Eq(val) = &range {
            let indexes = self.active_indexes(conn.as_mut(), graph)?;
            if indexes.contains(&format!("{label}:{property}")) {
                let vkey = value_to_index_key(val);
                let members: Vec<String> = redis::cmd("SMEMBERS")
                    .arg(self.kp(key_propidx(graph, label, property, &vkey)))
                    .query(conn.as_mut())
                    .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
                return members
                    .into_iter()
                    .map(|s| {
                        s.parse::<u64>()
                            .map(NodeId)
                            .map_err(|_| StorageError::Serialization {
                                msg: format!("bad node id in propidx: {s}"),
                            })
                    })
                    .collect();
            }
        }

        // Fallback: scan all nodes with the label and filter in-memory.
        let node_ids = self.find_nodes_by_label(graph, label)?;
        let mut result = Vec::new();
        for id in node_ids {
            let node = self.read_node(conn.as_mut(), graph, id.0)?;
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
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_reltype(graph, rel_type)))
            .query(conn.as_mut())
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
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_out(graph, node_id.0)))
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(Self::parse_adj_members(members))
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        let mut conn = self.conn()?;
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_in(graph, node_id.0)))
            .query(conn.as_mut())
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
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_nodes_set(graph)))
            .query(conn.as_mut())
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
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_edges_set(graph)))
            .query(conn.as_mut())
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

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        let prefix = format!("{}G:{{{graph}}}:label:", self.key_prefix);
        let pattern = format!("{prefix}*");
        let mut conn = self.conn()?;
        let mut cursor: u64 = 0;
        let mut labels = Vec::new();
        loop {
            let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(100u64)
                .query(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            for key in keys {
                if let Some(label) = key.strip_prefix(&prefix) {
                    labels.push(label.to_string());
                }
            }
            cursor = next;
            if cursor == 0 {
                break;
            }
        }
        Ok(labels)
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
            let raw_id: i64 = redis::cmd("INCRBY")
                .arg(self.kp(key_node_seq(graph)))
                .arg(1)
                .query(conn.as_mut())
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

        let node_key = self.kp(key_node(graph, id));
        let labels_json = serde_json::to_string(&labels)
            .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;
        let props_json = props_to_json(&clean_props)?;
        // Store node hash.
        redis::cmd("HSET")
            .arg(&node_key)
            .arg("labels")
            .arg(&labels_json)
            .arg("props")
            .arg(&props_json)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Update label indexes.
        for label in &labels {
            redis::cmd("SADD")
                .arg(self.kp(key_label(graph, label)))
                .arg(id.to_string())
                .query::<()>(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        }
        // Update all-nodes set.
        redis::cmd("SADD")
            .arg(self.kp(key_nodes_set(graph)))
            .arg(id.to_string())
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Maintain property indexes.
        self.index_node_properties(conn.as_mut(), graph, id, &labels, &clean_props)?;
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
        if self.read_node(conn.as_mut(), graph, src.0)?.is_none() {
            return Err(StorageError::NodeNotFound { id: src.0 });
        }
        if self.read_node(conn.as_mut(), graph, dst.0)?.is_none() {
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
            let raw_id: i64 = redis::cmd("INCRBY")
                .arg(self.kp(key_edge_seq(graph)))
                .arg(1)
                .query(conn.as_mut())
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

        let edge_key = self.kp(key_edge(graph, id));
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
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Update rel-type index.
        redis::cmd("SADD")
            .arg(self.kp(key_reltype(graph, &rel_type)))
            .arg(id.to_string())
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Update adjacency sets.
        let out_member = encode_adj(EdgeId(id), dst, &rel_type);
        let in_member = encode_adj(EdgeId(id), src, &rel_type);
        redis::cmd("SADD")
            .arg(self.kp(key_out(graph, src.0)))
            .arg(out_member)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        redis::cmd("SADD")
            .arg(self.kp(key_in(graph, dst.0)))
            .arg(in_member)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Update all-edges set.
        redis::cmd("SADD")
            .arg(self.kp(key_edges_set(graph)))
            .arg(id.to_string())
            .query::<()>(conn.as_mut())
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
            .read_node(conn.as_mut(), graph, id.0)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        // Deindex old property values before merging.
        self.deindex_node_properties(conn.as_mut(), graph, id.0, &node.labels, &node.properties)?;
        let mut merged = node.properties;
        merged.extend(properties);
        let props_json = props_to_json(&merged)?;
        redis::cmd("HSET")
            .arg(self.kp(key_node(graph, id.0)))
            .arg("props")
            .arg(&props_json)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Reindex with new values.
        self.index_node_properties(conn.as_mut(), graph, id.0, &node.labels, &merged)?;
        Ok(())
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let node = self
            .read_node(conn.as_mut(), graph, id.0)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        // Deindex old values.
        self.deindex_node_properties(conn.as_mut(), graph, id.0, &node.labels, &node.properties)?;
        let props_json = props_to_json(&properties)?;
        redis::cmd("HSET")
            .arg(self.kp(key_node(graph, id.0)))
            .arg("props")
            .arg(&props_json)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Index new values.
        self.index_node_properties(conn.as_mut(), graph, id.0, &node.labels, &properties)?;
        Ok(())
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let edge = self
            .read_edge(conn.as_mut(), graph, id.0)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        let mut merged = edge.properties;
        merged.extend(properties);
        let props_json = props_to_json(&merged)?;
        redis::cmd("HSET")
            .arg(self.kp(key_edge(graph, id.0)))
            .arg("props")
            .arg(&props_json)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        if self.read_edge(conn.as_mut(), graph, id.0)?.is_none() {
            return Err(StorageError::EdgeNotFound { id: id.0 });
        }
        let props_json = props_to_json(&properties)?;
        redis::cmd("HSET")
            .arg(self.kp(key_edge(graph, id.0)))
            .arg("props")
            .arg(&props_json)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let node = self
            .read_node(conn.as_mut(), graph, id.0)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        // Deindex old values before removal.
        self.deindex_node_properties(conn.as_mut(), graph, id.0, &node.labels, &node.properties)?;
        let mut props = node.properties;
        props.remove(key);
        let props_json = props_to_json(&props)?;
        redis::cmd("HSET")
            .arg(self.kp(key_node(graph, id.0)))
            .arg("props")
            .arg(&props_json)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Reindex with remaining values.
        self.index_node_properties(conn.as_mut(), graph, id.0, &node.labels, &props)?;
        Ok(())
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let edge = self
            .read_edge(conn.as_mut(), graph, id.0)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        let mut props = edge.properties;
        props.remove(key);
        let props_json = props_to_json(&props)?;
        redis::cmd("HSET")
            .arg(self.kp(key_edge(graph, id.0)))
            .arg("props")
            .arg(&props_json)
            .query::<()>(conn.as_mut())
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
            .read_node(conn.as_mut(), graph, id.0)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        let mut existing: std::collections::HashSet<String> = node.labels.into_iter().collect();
        for label in labels {
            if existing.insert(label.clone()) {
                // New label: update index.
                redis::cmd("SADD")
                    .arg(self.kp(key_label(graph, label)))
                    .arg(id.0.to_string())
                    .query::<()>(conn.as_mut())
                    .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            }
        }
        let new_labels: Vec<String> = existing.into_iter().collect();
        let labels_json = serde_json::to_string(&new_labels)
            .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;
        redis::cmd("HSET")
            .arg(self.kp(key_node(graph, id.0)))
            .arg("labels")
            .arg(&labels_json)
            .query::<()>(conn.as_mut())
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
            .read_node(conn.as_mut(), graph, id.0)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        let to_remove: std::collections::HashSet<&String> = labels.iter().collect();
        for label in &to_remove {
            redis::cmd("SREM")
                .arg(self.kp(key_label(graph, label)))
                .arg(id.0.to_string())
                .query::<()>(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        }
        let new_labels: Vec<String> = node
            .labels
            .into_iter()
            .filter(|l| !to_remove.contains(l))
            .collect();
        let labels_json = serde_json::to_string(&new_labels)
            .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;
        redis::cmd("HSET")
            .arg(self.kp(key_node(graph, id.0)))
            .arg("labels")
            .arg(&labels_json)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let node = self
            .read_node(conn.as_mut(), graph, id.0)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        // Check for relationships if not detaching.
        if !detach {
            let out: Vec<String> = redis::cmd("SMEMBERS")
                .arg(self.kp(key_out(graph, id.0)))
                .query(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            let in_: Vec<String> = redis::cmd("SMEMBERS")
                .arg(self.kp(key_in(graph, id.0)))
                .query(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            if !out.is_empty() || !in_.is_empty() {
                return Err(StorageError::Backend {
                    msg: format!("node {} has relationships; use DETACH DELETE", id.0),
                });
            }
        }
        if detach {
            // Delete all adjacent edges first, keeping the lock held throughout
            // to prevent new edges from arriving during the operation.
            let out_members: Vec<String> = redis::cmd("SMEMBERS")
                .arg(self.kp(key_out(graph, id.0)))
                .query(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            let in_members: Vec<String> = redis::cmd("SMEMBERS")
                .arg(self.kp(key_in(graph, id.0)))
                .query(conn.as_mut())
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
                self.delete_edge_inner(conn.as_mut(), graph, eid)?;
            }
        }
        // Remove property indexes.
        self.deindex_node_properties(conn.as_mut(), graph, id.0, &node.labels, &node.properties)?;
        // Remove label indexes.
        for label in &node.labels {
            redis::cmd("SREM")
                .arg(self.kp(key_label(graph, label)))
                .arg(id.0.to_string())
                .query::<()>(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        }
        // Remove from all-nodes set.
        redis::cmd("SREM")
            .arg(self.kp(key_nodes_set(graph)))
            .arg(id.0.to_string())
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Delete adjacency keys.
        redis::cmd("DEL")
            .arg(self.kp(key_out(graph, id.0)))
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        redis::cmd("DEL")
            .arg(self.kp(key_in(graph, id.0)))
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Delete node hash.
        redis::cmd("DEL")
            .arg(self.kp(key_node(graph, id.0)))
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(())
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        self.delete_edge_inner(conn.as_mut(), graph, id)
    }

    // ── Index management ──────────────────────────────────────────────────────

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        let mut conn = self.conn()?;
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_indexes(graph)))
            .query(conn.as_mut())
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
        // Record the index metadata.
        redis::cmd("SADD")
            .arg(self.kp(key_indexes(graph)))
            .arg(format!("{label}:{property}"))
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;

        // Build the index from existing nodes: scan all nodes with this label
        // and add entries for those that have the indexed property.
        let node_ids: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_label(graph, label)))
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;

        // Pipeline in batches for efficiency.
        const BATCH_SIZE: usize = 500;
        for chunk in node_ids.chunks(BATCH_SIZE) {
            // First, pipeline-read all nodes in this batch.
            let mut read_pipe = redis::pipe();
            for id_str in chunk {
                if let Ok(id) = id_str.parse::<u64>() {
                    read_pipe
                        .cmd("HMGET")
                        .arg(self.kp(key_node(graph, id)))
                        .arg("labels")
                        .arg("props");
                }
            }
            let raw: Vec<Vec<Option<String>>> = read_pipe
                .query(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;

            // Then pipeline-write all index entries.
            let mut write_pipe = redis::pipe();
            let mut has_cmds = false;
            for (i, fields) in raw.iter().enumerate() {
                if let (Some(Some(_)), Some(Some(props_json))) = (fields.first(), fields.get(1)) {
                    if let Ok(properties) = json_to_props(props_json) {
                        if let Some(val) = properties.get(property) {
                            if !val.is_null() {
                                let vkey = value_to_index_key(val);
                                write_pipe
                                    .cmd("SADD")
                                    .arg(self.kp(key_propidx(graph, label, property, &vkey)))
                                    .arg(&chunk[i])
                                    .ignore();
                                has_cmds = true;
                            }
                        }
                    }
                }
            }
            if has_cmds {
                write_pipe
                    .query::<()>(conn.as_mut())
                    .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            }
        }
        Ok(())
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        redis::cmd("SREM")
            .arg(self.kp(key_indexes(graph)))
            .arg(format!("{label}:{property}"))
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(())
    }

    // ── Graph administration ──────────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        validate_graph_name(name)?;
        let mut conn = self.conn()?;
        let added: i64 = redis::cmd("SADD")
            .arg(self.key_graphs())
            .arg(name)
            .query(conn.as_mut())
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
        redis::cmd("SREM")
            .arg(self.key_graphs())
            .arg(name)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Delete all keys with pattern G:{name}:* using SCAN.
        // We use SCAN with a match pattern to avoid blocking with KEYS.
        let pattern = format!("{}G:{{{name}}}:*", self.key_prefix);
        let mut cursor: u64 = 0;
        loop {
            let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(100u64)
                .query(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            if !keys.is_empty() {
                let mut del_cmd = redis::cmd("DEL");
                for k in &keys {
                    del_cmd.arg(k);
                }
                del_cmd
                    .query::<()>(conn.as_mut())
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
        redis::cmd("SISMEMBER")
            .arg(self.key_graphs())
            .arg(name)
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        let mut conn = self.conn()?;
        redis::cmd("SMEMBERS")
            .arg(self.key_graphs())
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        self.require_graph(graph)?;
        let mut conn = self.conn()?;
        let node_count: u64 = redis::cmd("SCARD")
            .arg(self.kp(key_nodes_set(graph)))
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        let edge_count: u64 = redis::cmd("SCARD")
            .arg(self.kp(key_edges_set(graph)))
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Count distinct labels and rel-types by scanning index keys.
        // Pattern: G:{graph}:label:*
        let label_pattern = format!("{}G:{{{graph}}}:label:*", self.key_prefix);
        let reltype_pattern = format!("{}G:{{{graph}}}:reltype:*", self.key_prefix);
        let label_count = scan_key_count(conn.as_mut(), &label_pattern)?;
        let rel_type_count = scan_key_count(conn.as_mut(), &reltype_pattern)?;
        Ok(GraphStats {
            node_count,
            edge_count,
            label_count: label_count as u32,
            rel_type_count: rel_type_count as u32,
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    // tx_snapshot and tx_commit use the default no-op implementations.
    // RedisGraphStore writes are immediately durable — no client-side
    // snapshot is needed. Redis Cluster mode can add WATCH/MULTI/EXEC
    // for per-slot transaction isolation in a future iteration.
}

/// Count the number of keys matching a pattern via SCAN.
fn scan_key_count(conn: &mut dyn ConnectionLike, pattern: &str) -> Result<usize, StorageError> {
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
            matches!(
                cmp_values(val, target),
                Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
            )
        }
        RangeQuery::Gt(target) => cmp_values(val, target) == Some(std::cmp::Ordering::Greater),
        RangeQuery::Gte(target) => {
            matches!(
                cmp_values(val, target),
                Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
            )
        }
        RangeQuery::Between { lo, hi, inclusive } => {
            let lo_ok = if *inclusive {
                matches!(
                    cmp_values(val, lo),
                    Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
                )
            } else {
                cmp_values(val, lo) == Some(std::cmp::Ordering::Greater)
            };
            let hi_ok = if *inclusive {
                matches!(
                    cmp_values(val, hi),
                    Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
                )
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
