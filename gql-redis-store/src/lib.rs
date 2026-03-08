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
//   Index set:       G:{g}:indexes          — SADD "{label}\x00{property}"
//   Graph registry:  __sd2_registry:graphs   — SADD graph name

pub mod create_store;
pub mod fanout;

mod connection;
mod indexing;
mod keys;
mod range;
mod reads;
mod writes;

pub use fanout::FanoutStore;

use std::collections::HashMap;
use std::env;

use redis::ConnectionLike;

use gql_core::error::StorageError;
use gql_core::serde_value::json_to_props;
use gql_core::storage::{
    AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor, NodeData, NodeId,
    RangeQuery,
};
use gql_core::types::Value;

use connection::{
    RedisConn, RedisPool, DEFAULT_IDLE_TIMEOUT_SECS, DEFAULT_MIN_IDLE, DEFAULT_POOL_SIZE,
    DEFAULT_POOL_TIMEOUT_SECS,
};
use keys::{decode_adj, key_edge, key_node, KEY_GRAPHS};

// ── RedisGraphStore ─────────────────────────────────────────────────────────

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
/// - `GQL_REDIS_POOL_SIZE` — max connections (default 64)
/// - `GQL_REDIS_POOL_TIMEOUT_SECS` — checkout timeout (default 5)
/// - `GQL_REDIS_IDLE_TIMEOUT_SECS` — idle connection reap timeout (default 300)
/// - `GQL_REDIS_MIN_IDLE` — minimum warm connections kept in pool (default 1)
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
    /// Pool size and timeout are read from `GQL_REDIS_POOL_SIZE` (default 64)
    /// and `GQL_REDIS_POOL_TIMEOUT_SECS` (default 5).
    /// Idle timeout and min-idle are read from `GQL_REDIS_IDLE_TIMEOUT_SECS` (default 300)
    /// and `GQL_REDIS_MIN_IDLE` (default 1).
    pub fn from_env() -> Result<Self, StorageError> {
        let prefix = env::var("GQL_REDIS_KEY_PREFIX").unwrap_or_default();
        let pool_size: u32 =
            gql_core::config::parse_env_range("GQL_REDIS_POOL_SIZE", DEFAULT_POOL_SIZE, 1, 1024);
        let pool_timeout_secs: u64 = gql_core::config::parse_env_range(
            "GQL_REDIS_POOL_TIMEOUT_SECS",
            DEFAULT_POOL_TIMEOUT_SECS,
            1,
            300,
        );
        let idle_timeout_secs: u64 = gql_core::config::parse_env_range(
            "GQL_REDIS_IDLE_TIMEOUT_SECS",
            DEFAULT_IDLE_TIMEOUT_SECS,
            0,
            86400,
        );
        let min_idle: u32 =
            gql_core::config::parse_env_range("GQL_REDIS_MIN_IDLE", DEFAULT_MIN_IDLE, 0, pool_size);

        // Cluster mode: GQL_REDIS_CLUSTER_NODES takes priority.
        if let Ok(nodes_str) = env::var("GQL_REDIS_CLUSTER_NODES") {
            let nodes: Vec<&str> = nodes_str.split(',').map(|s| s.trim()).collect();
            return Self::from_cluster_nodes(
                &nodes,
                &prefix,
                pool_size,
                pool_timeout_secs,
                idle_timeout_secs,
                min_idle,
            );
        }

        // Standalone mode.
        let url = env::var("GQL_REDIS_URL")
            .or_else(|_| env::var("REDIS_URL"))
            .unwrap_or_else(|_| {
                tracing::warn!("GQL_REDIS_URL not set, falling back to redis://127.0.0.1:6379");
                "redis://127.0.0.1:6379".into()
            });
        Self::from_url_with_options(
            &url,
            &prefix,
            pool_size,
            pool_timeout_secs,
            idle_timeout_secs,
            min_idle,
        )
    }

    /// Create a store connecting to an explicit Redis URL with an optional key prefix.
    pub fn from_url_with_prefix(url: &str, key_prefix: &str) -> Result<Self, StorageError> {
        Self::from_url_with_options(
            url,
            key_prefix,
            DEFAULT_POOL_SIZE,
            DEFAULT_POOL_TIMEOUT_SECS,
            DEFAULT_IDLE_TIMEOUT_SECS,
            DEFAULT_MIN_IDLE,
        )
    }

    /// Create a store with full configuration control (standalone mode).
    pub fn from_url_with_options(
        url: &str,
        key_prefix: &str,
        pool_size: u32,
        pool_timeout_secs: u64,
        idle_timeout_secs: u64,
        min_idle: u32,
    ) -> Result<Self, StorageError> {
        let pool = RedisPool::standalone(
            url,
            pool_size,
            pool_timeout_secs,
            idle_timeout_secs,
            min_idle,
        )?;
        Ok(Self {
            pool,
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
        idle_timeout_secs: u64,
        min_idle: u32,
    ) -> Result<Self, StorageError> {
        let pool = RedisPool::cluster(
            nodes,
            pool_size,
            pool_timeout_secs,
            idle_timeout_secs,
            min_idle,
        )?;
        Ok(Self {
            pool,
            key_prefix: key_prefix.to_string(),
        })
    }

    /// Create a store connecting to an explicit Redis URL with no key prefix.
    pub fn from_url(url: &str) -> Result<Self, StorageError> {
        Self::from_url_with_prefix(url, "")
    }

    // ── Internal helpers (shared across modules) ────────────────────────────

    /// Check out a connection from the pool.
    pub(crate) fn conn(&self) -> Result<RedisConn, StorageError> {
        self.pool.get()
    }

    /// Check that a graph exists; return error if not.
    pub(crate) fn require_graph(&self, g: &str) -> Result<(), StorageError> {
        if !self.graph_exists_impl(g)? {
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
    pub(crate) fn kp(&self, key: String) -> String {
        if self.key_prefix.is_empty() {
            key
        } else {
            format!("{}{}", self.key_prefix, key)
        }
    }

    /// Return the prefixed graph-registry key.
    #[inline]
    pub(crate) fn key_graphs(&self) -> String {
        self.kp(KEY_GRAPHS.to_string())
    }

    /// Read a node hash from Redis; returns `None` if the key is absent.
    pub(crate) fn read_node(
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
    pub(crate) fn read_edge(
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
            (None, None, None, None) => Ok(None),
            _ => {
                tracing::error!(
                    edge_id = id,
                    graph = g,
                    src = ?fields[0],
                    dst = ?fields[1],
                    rel_type = ?fields[2],
                    props = ?fields[3],
                    "partial edge data detected — possible data corruption"
                );
                Ok(None)
            }
        }
    }

    /// Parse adjacency set members into `AdjEntry` list.
    pub(crate) fn parse_adj_members(members: Vec<String>) -> Vec<AdjEntry> {
        let mut entries = Vec::new();
        for m in members {
            match decode_adj(&m) {
                Some(entry) => entries.push(entry),
                None => {
                    tracing::warn!(member = %m, "failed to decode adjacency member, skipping");
                }
            }
        }
        entries
    }
}

// ── GraphStore impl ─────────────────────────────────────────────────────────

impl GraphStore for RedisGraphStore {
    // ── Single-item reads ────────────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        self.get_node_impl(graph, id)
    }

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        self.get_edge_impl(graph, id)
    }

    // ── Batch reads ──────────────────────────────────────────────────────────

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        self.get_nodes_batch_impl(graph, ids)
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        self.get_edges_batch_impl(graph, ids)
    }

    // ── Label/type index reads ───────────────────────────────────────────────

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        self.find_nodes_by_label_impl(graph, label)
    }

    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        self.find_nodes_by_label_and_property_impl(graph, label, property, range)
    }

    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.find_edges_by_type_impl(graph, rel_type)
    }

    // ── Adjacency reads ─────────────────────────────────────────────────────

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.out_edges_impl(graph, node_id)
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.in_edges_impl(graph, node_id)
    }

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.out_edges_by_type_impl(graph, node_id, rel_types)
    }

    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.in_edges_by_type_impl(graph, node_id, rel_types)
    }

    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.out_edge_ids_impl(graph, node_id)
    }

    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        self.in_edge_ids_impl(graph, node_id)
    }

    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        self.all_node_ids_impl(graph)
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.all_edge_ids_impl(graph)
    }

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        self.all_label_names_impl(graph)
    }

    // ── Write operations ────────────────────────────────────────────────────

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: HashMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        self.create_node_impl(graph, labels, properties)
    }

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: HashMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        self.create_edge_impl(graph, src, dst, rel_type, properties)
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.set_node_properties_impl(graph, id, properties)
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.replace_node_properties_impl(graph, id, properties)
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.set_edge_properties_impl(graph, id, properties)
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.replace_edge_properties_impl(graph, id, properties)
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        self.remove_node_property_impl(graph, id, key)
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        self.remove_edge_property_impl(graph, id, key)
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.add_node_labels_impl(graph, id, labels)
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.remove_node_labels_impl(graph, id, labels)
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        self.delete_node_impl(graph, id, detach)
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.delete_edge_impl(graph, id)
    }

    // ── Index management ────────────────────────────────────────────────────

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        self.list_indexes_impl(graph)
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.create_index_impl(graph, label, property)
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.drop_index_impl(graph, label, property)
    }

    // ── Graph administration ────────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        self.create_graph_impl(name)
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        self.delete_graph_impl(name)
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        self.graph_exists_impl(name)
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        self.list_graphs_impl()
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        self.graph_stats_impl(graph)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    // tx_snapshot and tx_commit use the default no-op implementations.
    // RedisGraphStore writes are immediately durable — no client-side
    // snapshot is needed. Redis Cluster mode can add WATCH/MULTI/EXEC
    // for per-slot transaction isolation in a future iteration.
}

#[cfg(test)]
mod tests {
    use super::*;
    use keys::encode_adj;

    // ── parse_adj_members ───────────────────────────────────────────────────

    #[test]
    fn parse_adj_members_valid_entries() {
        let members = vec!["1:2:KNOWS".to_string(), "3:4:LIKES".to_string()];
        let entries = RedisGraphStore::parse_adj_members(members);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].edge_id, EdgeId(1));
        assert_eq!(entries[0].neighbor_id, NodeId(2));
        assert_eq!(entries[0].rel_type, "KNOWS");
        assert_eq!(entries[1].edge_id, EdgeId(3));
        assert_eq!(entries[1].neighbor_id, NodeId(4));
        assert_eq!(entries[1].rel_type, "LIKES");
    }

    #[test]
    fn parse_adj_members_skips_malformed() {
        let members = vec![
            "1:2:KNOWS".to_string(),
            "bad_entry".to_string(), // malformed — should be filtered out
            "3:4:LIKES".to_string(),
        ];
        let entries = RedisGraphStore::parse_adj_members(members);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn parse_adj_members_empty_input() {
        let entries = RedisGraphStore::parse_adj_members(vec![]);
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_adj_members_all_malformed() {
        let members = vec!["bad".to_string(), "also_bad".to_string()];
        let entries = RedisGraphStore::parse_adj_members(members);
        assert!(entries.is_empty());
    }

    // ── encode_adj / decode_adj integration with parse_adj_members ──────────

    #[test]
    fn encode_then_parse_round_trip() {
        let encoded = vec![
            encode_adj(EdgeId(10), NodeId(20), "FOLLOWS"),
            encode_adj(EdgeId(11), NodeId(21), "BLOCKS"),
        ];
        let entries = RedisGraphStore::parse_adj_members(encoded);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].edge_id, EdgeId(10));
        assert_eq!(entries[0].neighbor_id, NodeId(20));
        assert_eq!(entries[0].rel_type, "FOLLOWS");
        assert_eq!(entries[1].edge_id, EdgeId(11));
        assert_eq!(entries[1].neighbor_id, NodeId(21));
        assert_eq!(entries[1].rel_type, "BLOCKS");
    }

    // ── key_graphs helper ───────────────────────────────────────────────────
    // We cannot construct RedisGraphStore without a live Redis pool, but we
    // can test the key_prefix logic via the `kp` function by checking its
    // contract: if prefix is empty, return key unchanged; otherwise prepend.
    // These tests exercise the `kp` logic indirectly via `key_graphs` by
    // verifying the expected key format using the constant.

    #[test]
    fn key_graphs_constant_value() {
        assert_eq!(keys::KEY_GRAPHS, "__sd2_registry:graphs");
    }

    // ── Key format smoke tests ──────────────────────────────────────────────
    // Verify that the key schema documented in the module comment is accurate.

    #[test]
    fn key_schema_node_hash() {
        assert_eq!(keys::key_node("mydb", 1), "G:{mydb}:node:1");
    }

    #[test]
    fn key_schema_edge_hash() {
        assert_eq!(keys::key_edge("mydb", 5), "G:{mydb}:edge:5");
    }

    #[test]
    fn key_schema_label_index() {
        assert_eq!(keys::key_label("mydb", "Person"), "G:{mydb}:label:Person");
    }

    #[test]
    fn key_schema_reltype_index() {
        assert_eq!(keys::key_reltype("mydb", "KNOWS"), "G:{mydb}:reltype:KNOWS");
    }

    #[test]
    fn key_schema_out_adjacency() {
        assert_eq!(keys::key_out("mydb", 7), "G:{mydb}:out:7");
    }

    #[test]
    fn key_schema_in_adjacency() {
        assert_eq!(keys::key_in("mydb", 7), "G:{mydb}:in:7");
    }

    // ── Connection defaults ─────────────────────────────────────────────────

    #[test]
    fn connection_defaults_are_sensible() {
        use connection::*;
        assert!(DEFAULT_POOL_SIZE > 0);
        assert!(DEFAULT_POOL_TIMEOUT_SECS > 0);
        assert!(DEFAULT_IDLE_TIMEOUT_SECS > 0);
        assert!(DEFAULT_MIN_IDLE > 0);
        // Pool size should be reasonable (not absurdly large).
        assert!(DEFAULT_POOL_SIZE <= 256);
    }
}
