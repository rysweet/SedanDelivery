// module_store.rs — GraphStore implementation that uses the Valkey Module API
// (ctx.call()) to execute Redis commands internally, avoiding the TCP loopback
// deadlock that prevents RedisGraphStore from working inside a module.
//
// This store uses the EXACT same key schema as RedisGraphStore so that the Bolt
// server, REST API, and GQL module all see the same graph data.
//
// Thread-safety: Valkey executes module commands on a single thread per client.
// The module context pointer is valid for the duration of one command handler.
// We stash it in a thread-local before each command and clear it afterward.

use std::any::Any;
use std::cell::RefCell;

use indexmap::IndexMap;
use redis_module::{Context, RedisValue};

use cypher_core::error::StorageError;
use cypher_core::serde_value::{json_to_props, props_to_json};
use cypher_core::storage::{
    AdjEntry, ConstraintDescriptor, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor,
    NodeData, NodeId, RangeQuery,
};
use cypher_core::types::Value;

// ── Thread-local context stash ──────────────────────────────────────────────

thread_local! {
    static MODULE_CTX: RefCell<Option<*const Context>> = const { RefCell::new(None) };
}

/// A `GraphStore` implementation backed by the Valkey Module API.
///
/// Instead of making TCP calls to Redis (which would deadlock inside a module),
/// this store calls Redis commands via `ctx.call()` — the internal dispatch path
/// that the Valkey module API provides.
///
/// The module context must be set before any GraphStore method is called, and
/// cleared afterward. This is done by the command handlers in lib.rs.
pub struct ModuleRedisStore {
    key_prefix: String,
}

impl ModuleRedisStore {
    pub fn new() -> Self {
        Self {
            key_prefix: String::new(),
        }
    }

    /// Set the module context for the current command invocation.
    /// Must be called at the start of every command handler.
    pub fn set_context(ctx: &Context) {
        MODULE_CTX.with(|cell| {
            *cell.borrow_mut() = Some(ctx as *const _);
        });
    }

    /// Clear the module context. Must be called at the end of every command handler.
    pub fn clear_context() {
        MODULE_CTX.with(|cell| {
            *cell.borrow_mut() = None;
        });
    }

    /// Execute a closure with access to the current module context.
    fn with_ctx<F, R>(&self, f: F) -> Result<R, StorageError>
    where
        F: FnOnce(&Context) -> Result<R, StorageError>,
    {
        MODULE_CTX.with(|cell| {
            let ptr = cell.borrow();
            let ctx_ptr = ptr.ok_or(StorageError::Backend {
                msg: "ModuleRedisStore: no module context set".into(),
            })?;
            // SAFETY: the context pointer is valid for the duration of the
            // command handler, and we only use it within that scope. Valkey
            // dispatches module commands on a single thread, so there is no
            // concurrent access.
            let ctx = unsafe { &*ctx_ptr };
            f(ctx)
        })
    }

    /// Apply the configured key prefix to a key. No-op when prefix is empty.
    #[inline]
    fn kp(&self, key: String) -> String {
        if self.key_prefix.is_empty() {
            key
        } else {
            format!("{}{}", self.key_prefix, key)
        }
    }

    // ── ctx.call() helpers ──────────────────────────────────────────────────

    /// Execute a Redis command via the module API and return the RedisValue.
    fn call_cmd(
        &self,
        ctx: &Context,
        cmd: &str,
        args: &[&str],
    ) -> Result<RedisValue, StorageError> {
        ctx.call(cmd, args).map_err(|e| StorageError::Backend {
            msg: format!("ctx.call({cmd}) failed: {e:?}"),
        })
    }

    /// Extract an i64 from a RedisValue::Integer reply.
    fn expect_integer(val: &RedisValue, cmd: &str) -> Result<i64, StorageError> {
        match val {
            RedisValue::Integer(i) => Ok(*i),
            other => Err(StorageError::Backend {
                msg: format!("expected integer from {cmd}, got: {other:?}"),
            }),
        }
    }

    /// Extract a bulk string from a RedisValue reply, returning None for Null.
    fn maybe_bulk_string(val: &RedisValue) -> Option<String> {
        match val {
            RedisValue::BulkString(s) => Some(s.clone()),
            RedisValue::SimpleString(s) => Some(s.clone()),
            RedisValue::SimpleStringStatic(s) => Some(s.to_string()),
            _ => None,
        }
    }

    /// Extract an array of strings from a RedisValue::Array reply (SMEMBERS result).
    fn extract_string_array(val: &RedisValue) -> Vec<String> {
        match val {
            RedisValue::Array(arr) => arr.iter().filter_map(Self::maybe_bulk_string).collect(),
            _ => Vec::new(),
        }
    }

    /// Extract fields from an HMGET-style array reply.
    /// Returns a Vec<Option<String>> matching the requested fields.
    fn extract_hmget_fields(val: &RedisValue) -> Vec<Option<String>> {
        match val {
            RedisValue::Array(arr) => arr.iter().map(Self::maybe_bulk_string).collect(),
            _ => Vec::new(),
        }
    }

    // ── Node/edge parsing from Redis replies ─────────────────────────────────

    fn parse_node_from_hmget(
        &self,
        id: NodeId,
        fields: &[Option<String>],
    ) -> Result<Option<NodeData>, StorageError> {
        if fields.len() < 2 {
            return Ok(None);
        }
        match (&fields[0], &fields[1]) {
            (Some(labels_json), Some(props_json)) => {
                let labels: Vec<String> =
                    serde_json::from_str(labels_json).map_err(|e| StorageError::Serialization {
                        msg: format!("bad labels JSON: {e}"),
                    })?;
                let properties = json_to_props(props_json)?;
                Ok(Some(NodeData {
                    id,
                    labels,
                    properties,
                }))
            }
            _ => Ok(None),
        }
    }

    fn parse_edge_from_hmget(
        &self,
        id: EdgeId,
        fields: &[Option<String>],
    ) -> Result<Option<EdgeData>, StorageError> {
        if fields.len() < 4 {
            return Ok(None);
        }
        match (&fields[0], &fields[1], &fields[2], &fields[3]) {
            (Some(src_s), Some(dst_s), Some(rel_type), Some(props_json)) => {
                let src =
                    NodeId(
                        src_s
                            .parse::<u64>()
                            .map_err(|_| StorageError::Serialization {
                                msg: format!("bad src in edge: {src_s}"),
                            })?,
                    );
                let dst =
                    NodeId(
                        dst_s
                            .parse::<u64>()
                            .map_err(|_| StorageError::Serialization {
                                msg: format!("bad dst in edge: {dst_s}"),
                            })?,
                    );
                let properties = json_to_props(props_json)?;
                Ok(Some(EdgeData {
                    id,
                    rel_type: rel_type.clone(),
                    src,
                    dst,
                    properties,
                }))
            }
            _ => Ok(None),
        }
    }

    /// Parse adjacency members from SMEMBERS results.
    fn parse_adj_members(members: &[String]) -> Vec<AdjEntry> {
        members.iter().filter_map(|m| decode_adj(m)).collect()
    }
}

// ── Key schema (duplicated from sd2-redis-store/src/keys.rs) ──────────────
//
// These are trivial one-liners. We duplicate them here to avoid pulling the
// redis-module crate into sd2-redis-store, which the project architecture
// explicitly forbids. The key formats MUST match RedisGraphStore exactly.

const KEY_GRAPHS: &str = "__sd2_registry:graphs";

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

fn key_graph_deleting(g: &str) -> String {
    format!("G:{{{g}}}:_deleting")
}

fn key_label_names(g: &str) -> String {
    format!("G:{{{g}}}:_labels")
}

fn key_reltype_names(g: &str) -> String {
    format!("G:{{{g}}}:_reltypes")
}

/// Encode an adjacency set member: "{edge_id}:{neighbor_id}:{rel_type}"
fn encode_adj(edge_id: EdgeId, neighbor_id: NodeId, rel_type: &str) -> String {
    format!("{}:{}:{}", edge_id.0, neighbor_id.0, rel_type)
}

/// Decode an adjacency set member. Returns `None` on malformed input.
fn decode_adj(s: &str) -> Option<AdjEntry> {
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

// ── GraphStore implementation ───────────────────────────────────────────────

impl GraphStore for ModuleRedisStore {
    // ── Single-item reads ───────────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_node(graph, id.0));
            let result = self.call_cmd(ctx, "HMGET", &[&key, "labels", "props"])?;
            let fields = Self::extract_hmget_fields(&result);
            self.parse_node_from_hmget(id, &fields)
        })
    }

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_edge(graph, id.0));
            let result = self.call_cmd(ctx, "HMGET", &[&key, "src", "dst", "rel_type", "props"])?;
            let fields = Self::extract_hmget_fields(&result);
            self.parse_edge_from_hmget(id, &fields)
        })
    }

    // ── Batch reads ─────────────────────────────────────────────────────────
    // Fall through to default loop-based implementations.

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        ids.iter().map(|id| self.get_node(graph, *id)).collect()
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        ids.iter().map(|id| self.get_edge(graph, *id)).collect()
    }

    // ── Label/type index reads ──────────────────────────────────────────────

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_label(graph, label));
            let result = self.call_cmd(ctx, "SMEMBERS", &[&key])?;
            Self::extract_string_array(&result)
                .into_iter()
                .map(|s| {
                    s.parse::<u64>()
                        .map(NodeId)
                        .map_err(|_| StorageError::Serialization {
                            msg: format!("bad node id in label index: {s}"),
                        })
                })
                .collect()
        })
    }

    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        // No property index support in module store — fallback to full scan + filter.
        let node_ids = self.find_nodes_by_label(graph, label)?;
        let mut result = Vec::new();
        for id in node_ids {
            if let Some(node) = self.get_node(graph, id)? {
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
        self.with_ctx(|ctx| {
            let key = self.kp(key_reltype(graph, rel_type));
            let result = self.call_cmd(ctx, "SMEMBERS", &[&key])?;
            Self::extract_string_array(&result)
                .into_iter()
                .map(|s| {
                    s.parse::<u64>()
                        .map(EdgeId)
                        .map_err(|_| StorageError::Serialization {
                            msg: format!("bad edge id in reltype index: {s}"),
                        })
                })
                .collect()
        })
    }

    // ── Adjacency reads ─────────────────────────────────────────────────────

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_out(graph, node_id.0));
            let result = self.call_cmd(ctx, "SMEMBERS", &[&key])?;
            let members = Self::extract_string_array(&result);
            Ok(Self::parse_adj_members(&members))
        })
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_in(graph, node_id.0));
            let result = self.call_cmd(ctx, "SMEMBERS", &[&key])?;
            let members = Self::extract_string_array(&result);
            Ok(Self::parse_adj_members(&members))
        })
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
        self.with_ctx(|ctx| {
            let key = self.kp(key_nodes_set(graph));
            let result = self.call_cmd(ctx, "SMEMBERS", &[&key])?;
            Self::extract_string_array(&result)
                .into_iter()
                .map(|s| {
                    s.parse::<u64>()
                        .map(NodeId)
                        .map_err(|_| StorageError::Serialization {
                            msg: format!("bad node id in nodes set: {s}"),
                        })
                })
                .collect()
        })
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_edges_set(graph));
            let result = self.call_cmd(ctx, "SMEMBERS", &[&key])?;
            Self::extract_string_array(&result)
                .into_iter()
                .map(|s| {
                    s.parse::<u64>()
                        .map(EdgeId)
                        .map_err(|_| StorageError::Serialization {
                            msg: format!("bad edge id in edges set: {s}"),
                        })
                })
                .collect()
        })
    }

    // ── Write operations ────────────────────────────────────────────────────

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        self.with_ctx(|ctx| {
            // Allocate ID.
            let seq_key = self.kp(key_node_seq(graph));
            let result = self.call_cmd(ctx, "INCRBY", &[&seq_key, "1"])?;
            let raw_id = Self::expect_integer(&result, "INCRBY")?;
            if raw_id <= 0 {
                return Err(StorageError::IdOverflow {
                    graph: graph.to_string(),
                });
            }
            let id = raw_id as u64;

            // Serialize.
            let labels_json = serde_json::to_string(&labels)
                .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;
            let props_json = props_to_json(&properties)?;

            // HSET node hash.
            let node_key = self.kp(key_node(graph, id));
            self.call_cmd(
                ctx,
                "HSET",
                &[&node_key, "labels", &labels_json, "props", &props_json],
            )?;

            // SADD to all-nodes set.
            let nodes_set_key = self.kp(key_nodes_set(graph));
            let id_str = id.to_string();
            self.call_cmd(ctx, "SADD", &[&nodes_set_key, &id_str])?;

            // SADD to each label index.
            for label in &labels {
                let label_key = self.kp(key_label(graph, label));
                self.call_cmd(ctx, "SADD", &[&label_key, &id_str])?;
            }

            // SADD label names to registry.
            if !labels.is_empty() {
                let registry_key = self.kp(key_label_names(graph));
                // ctx.call expects &[&str], build dynamically.
                let mut args: Vec<&str> = vec![&registry_key];
                for label in &labels {
                    args.push(label.as_str());
                }
                self.call_cmd(ctx, "SADD", &args)?;
            }

            Ok(NodeId(id))
        })
    }

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        self.with_ctx(|ctx| {
            // Verify src and dst exist.
            let src_key = self.kp(key_node(graph, src.0));
            let src_check = self.call_cmd(ctx, "EXISTS", &[&src_key])?;
            if Self::expect_integer(&src_check, "EXISTS")? == 0 {
                return Err(StorageError::NodeNotFound { id: src.0 });
            }
            let dst_key = self.kp(key_node(graph, dst.0));
            let dst_check = self.call_cmd(ctx, "EXISTS", &[&dst_key])?;
            if Self::expect_integer(&dst_check, "EXISTS")? == 0 {
                return Err(StorageError::NodeNotFound { id: dst.0 });
            }

            // Allocate ID.
            let seq_key = self.kp(key_edge_seq(graph));
            let result = self.call_cmd(ctx, "INCRBY", &[&seq_key, "1"])?;
            let raw_id = Self::expect_integer(&result, "INCRBY")?;
            if raw_id <= 0 {
                return Err(StorageError::IdOverflow {
                    graph: graph.to_string(),
                });
            }
            let id = raw_id as u64;
            let id_str = id.to_string();

            // Serialize.
            let props_json = props_to_json(&properties)?;

            // HSET edge hash.
            let edge_key = self.kp(key_edge(graph, id));
            let src_str = src.0.to_string();
            let dst_str = dst.0.to_string();
            self.call_cmd(
                ctx,
                "HSET",
                &[
                    &edge_key,
                    "src",
                    &src_str,
                    "dst",
                    &dst_str,
                    "rel_type",
                    &rel_type,
                    "props",
                    &props_json,
                ],
            )?;

            // SADD to all-edges set.
            let edges_set_key = self.kp(key_edges_set(graph));
            self.call_cmd(ctx, "SADD", &[&edges_set_key, &id_str])?;

            // SADD to reltype index.
            let reltype_key = self.kp(key_reltype(graph, &rel_type));
            self.call_cmd(ctx, "SADD", &[&reltype_key, &id_str])?;

            // SADD reltype name to registry.
            let reltype_names_key = self.kp(key_reltype_names(graph));
            self.call_cmd(ctx, "SADD", &[&reltype_names_key, &rel_type])?;

            // SADD out-adjacency.
            let out_key = self.kp(key_out(graph, src.0));
            let out_member = encode_adj(EdgeId(id), dst, &rel_type);
            self.call_cmd(ctx, "SADD", &[&out_key, &out_member])?;

            // SADD in-adjacency.
            let in_key = self.kp(key_in(graph, dst.0));
            let in_member = encode_adj(EdgeId(id), src, &rel_type);
            self.call_cmd(ctx, "SADD", &[&in_key, &in_member])?;

            Ok(EdgeId(id))
        })
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        let node = self
            .get_node(graph, id)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        let mut merged = node.properties;
        merged.extend(properties);
        let props_json = props_to_json(&merged)?;
        self.with_ctx(|ctx| {
            let node_key = self.kp(key_node(graph, id.0));
            self.call_cmd(ctx, "HSET", &[&node_key, "props", &props_json])?;
            Ok(())
        })
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        // Verify node exists.
        if self.get_node(graph, id)?.is_none() {
            return Err(StorageError::NodeNotFound { id: id.0 });
        }
        let props_json = props_to_json(&properties)?;
        self.with_ctx(|ctx| {
            let node_key = self.kp(key_node(graph, id.0));
            self.call_cmd(ctx, "HSET", &[&node_key, "props", &props_json])?;
            Ok(())
        })
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        let edge = self
            .get_edge(graph, id)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        let mut merged = edge.properties;
        merged.extend(properties);
        let props_json = props_to_json(&merged)?;
        self.with_ctx(|ctx| {
            let edge_key = self.kp(key_edge(graph, id.0));
            self.call_cmd(ctx, "HSET", &[&edge_key, "props", &props_json])?;
            Ok(())
        })
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        if self.get_edge(graph, id)?.is_none() {
            return Err(StorageError::EdgeNotFound { id: id.0 });
        }
        let props_json = props_to_json(&properties)?;
        self.with_ctx(|ctx| {
            let edge_key = self.kp(key_edge(graph, id.0));
            self.call_cmd(ctx, "HSET", &[&edge_key, "props", &props_json])?;
            Ok(())
        })
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        let node = self
            .get_node(graph, id)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;
        let mut props = node.properties;
        props.shift_remove(key);
        let props_json = props_to_json(&props)?;
        self.with_ctx(|ctx| {
            let node_key = self.kp(key_node(graph, id.0));
            self.call_cmd(ctx, "HSET", &[&node_key, "props", &props_json])?;
            Ok(())
        })
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        let edge = self
            .get_edge(graph, id)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        let mut props = edge.properties;
        props.shift_remove(key);
        let props_json = props_to_json(&props)?;
        self.with_ctx(|ctx| {
            let edge_key = self.kp(key_edge(graph, id.0));
            self.call_cmd(ctx, "HSET", &[&edge_key, "props", &props_json])?;
            Ok(())
        })
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let node = self
            .get_node(graph, id)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;

        let existing: std::collections::HashSet<&String> = node.labels.iter().collect();
        let new_labels: Vec<&String> = labels.iter().filter(|l| !existing.contains(l)).collect();
        if new_labels.is_empty() {
            return Ok(());
        }

        // Merge label lists.
        let mut all_labels = node.labels.clone();
        for label in &new_labels {
            all_labels.push((*label).clone());
        }
        let labels_json = serde_json::to_string(&all_labels)
            .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;

        self.with_ctx(|ctx| {
            // Update node hash.
            let node_key = self.kp(key_node(graph, id.0));
            self.call_cmd(ctx, "HSET", &[&node_key, "labels", &labels_json])?;

            // SADD to label indexes.
            let id_str = id.0.to_string();
            for label in &new_labels {
                let label_key = self.kp(key_label(graph, label));
                self.call_cmd(ctx, "SADD", &[&label_key, &id_str])?;
            }

            // Update label registry.
            let registry_key = self.kp(key_label_names(graph));
            let label_strs: Vec<&str> = new_labels.iter().map(|l| l.as_str()).collect();
            let mut args: Vec<&str> = vec![&registry_key];
            args.extend(label_strs);
            self.call_cmd(ctx, "SADD", &args)?;

            Ok(())
        })
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let node = self
            .get_node(graph, id)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;

        let to_remove: std::collections::HashSet<&String> = labels.iter().collect();
        let removed: Vec<String> = node
            .labels
            .iter()
            .filter(|l| to_remove.contains(l))
            .cloned()
            .collect();
        if removed.is_empty() {
            return Ok(());
        }

        let new_labels: Vec<String> = node
            .labels
            .into_iter()
            .filter(|l| !to_remove.contains(l))
            .collect();
        let labels_json = serde_json::to_string(&new_labels)
            .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;

        self.with_ctx(|ctx| {
            let node_key = self.kp(key_node(graph, id.0));
            self.call_cmd(ctx, "HSET", &[&node_key, "labels", &labels_json])?;

            let id_str = id.0.to_string();
            for label in &removed {
                let label_key = self.kp(key_label(graph, label));
                self.call_cmd(ctx, "SREM", &[&label_key, &id_str])?;
            }
            Ok(())
        })
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        let node = self
            .get_node(graph, id)?
            .ok_or(StorageError::NodeNotFound { id: id.0 })?;

        // Check for edges if not detaching.
        if !detach {
            let out = self.out_edges(graph, id)?;
            let in_ = self.in_edges(graph, id)?;
            if !out.is_empty() || !in_.is_empty() {
                return Err(StorageError::Backend {
                    msg: format!("node {} has relationships; use DETACH DELETE", id.0),
                });
            }
        }

        // Detach: delete all adjacent edges first.
        if detach {
            let mut edge_ids: std::collections::HashSet<EdgeId> = std::collections::HashSet::new();
            for entry in self.out_edges(graph, id)? {
                edge_ids.insert(entry.edge_id);
            }
            for entry in self.in_edges(graph, id)? {
                edge_ids.insert(entry.edge_id);
            }
            for eid in edge_ids {
                self.delete_edge(graph, eid)?;
            }
        }

        self.with_ctx(|ctx| {
            let id_str = id.0.to_string();

            // SREM from label indexes.
            for label in &node.labels {
                let label_key = self.kp(key_label(graph, label));
                self.call_cmd(ctx, "SREM", &[&label_key, &id_str])?;
            }

            // SREM from all-nodes set.
            let nodes_set_key = self.kp(key_nodes_set(graph));
            self.call_cmd(ctx, "SREM", &[&nodes_set_key, &id_str])?;

            // DEL adjacency keys.
            let out_key = self.kp(key_out(graph, id.0));
            self.call_cmd(ctx, "DEL", &[&out_key])?;
            let in_key = self.kp(key_in(graph, id.0));
            self.call_cmd(ctx, "DEL", &[&in_key])?;

            // DEL node hash.
            let node_key = self.kp(key_node(graph, id.0));
            self.call_cmd(ctx, "DEL", &[&node_key])?;

            Ok(())
        })
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        let edge = self
            .get_edge(graph, id)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;

        self.with_ctx(|ctx| {
            let id_str = id.0.to_string();

            // SREM from reltype index.
            let reltype_key = self.kp(key_reltype(graph, &edge.rel_type));
            self.call_cmd(ctx, "SREM", &[&reltype_key, &id_str])?;

            // SREM from all-edges set.
            let edges_set_key = self.kp(key_edges_set(graph));
            self.call_cmd(ctx, "SREM", &[&edges_set_key, &id_str])?;

            // SREM out-adjacency.
            let out_key = self.kp(key_out(graph, edge.src.0));
            let out_member = encode_adj(id, edge.dst, &edge.rel_type);
            self.call_cmd(ctx, "SREM", &[&out_key, &out_member])?;

            // SREM in-adjacency.
            let in_key = self.kp(key_in(graph, edge.dst.0));
            let in_member = encode_adj(id, edge.src, &edge.rel_type);
            self.call_cmd(ctx, "SREM", &[&in_key, &in_member])?;

            // DEL edge hash.
            let edge_key = self.kp(key_edge(graph, id.0));
            self.call_cmd(ctx, "DEL", &[&edge_key])?;

            Ok(())
        })
    }

    // ── Index management ────────────────────────────────────────────────────

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_indexes(graph));
            let result = self.call_cmd(ctx, "SMEMBERS", &[&key])?;
            let members = Self::extract_string_array(&result);
            Ok(members
                .into_iter()
                .filter_map(|spec| {
                    spec.split_once('\x00')
                        .map(|(label, property)| IndexDescriptor {
                            label: label.to_string(),
                            property: property.to_string(),
                        })
                })
                .collect())
        })
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_indexes(graph));
            let member = format!("{label}\x00{property}");
            self.call_cmd(ctx, "SADD", &[&key, &member])?;
            Ok(())
        })
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_indexes(graph));
            let member = format!("{label}\x00{property}");
            self.call_cmd(ctx, "SREM", &[&key, &member])?;
            Ok(())
        })
    }

    // ── Constraint management — not supported ───────────────────────────────

    fn list_constraints(&self, _graph: &str) -> Result<Vec<ConstraintDescriptor>, StorageError> {
        Ok(vec![])
    }

    // ── Graph administration ────────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        cypher_core::storage::validate_graph_name(name)?;
        self.with_ctx(|ctx| {
            // Check if graph is being deleted.
            let deleting_key = self.kp(key_graph_deleting(name));
            let exists = self.call_cmd(ctx, "EXISTS", &[&deleting_key])?;
            if Self::expect_integer(&exists, "EXISTS")? > 0 {
                return Err(StorageError::TransactionConflict);
            }

            let graphs_key = self.kp(KEY_GRAPHS.to_string());
            let result = self.call_cmd(ctx, "SADD", &[&graphs_key, name])?;
            let added = Self::expect_integer(&result, "SADD")?;
            if added == 0 {
                return Err(StorageError::GraphAlreadyExists {
                    name: name.to_string(),
                });
            }
            Ok(())
        })
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        // Simplified delete: remove the graph from the registry and clean up
        // known keys. For large graphs, a more incremental approach is needed.
        self.with_ctx(|ctx| {
            // Verify graph exists.
            let graphs_key = self.kp(KEY_GRAPHS.to_string());
            let check = self.call_cmd(ctx, "SISMEMBER", &[&graphs_key, name])?;
            let is_member = Self::expect_integer(&check, "SISMEMBER")?;

            let deleting_key = self.kp(key_graph_deleting(name));
            let retrying = {
                let r = self.call_cmd(ctx, "EXISTS", &[&deleting_key])?;
                Self::expect_integer(&r, "EXISTS")? > 0
            };

            if is_member == 0 && !retrying {
                return Err(StorageError::GraphNotFound {
                    name: name.to_string(),
                });
            }

            // Set deleting marker.
            self.call_cmd(ctx, "SET", &[&deleting_key, "1"])?;

            // Enumerate all nodes and edges.
            let nodes_key = self.kp(key_nodes_set(name));
            let node_result = self.call_cmd(ctx, "SMEMBERS", &[&nodes_key])?;
            let node_ids = Self::extract_string_array(&node_result);

            let edges_key = self.kp(key_edges_set(name));
            let edge_result = self.call_cmd(ctx, "SMEMBERS", &[&edges_key])?;
            let edge_ids = Self::extract_string_array(&edge_result);

            // Collect label and reltype names.
            let labels_reg_key = self.kp(key_label_names(name));
            let labels_result = self.call_cmd(ctx, "SMEMBERS", &[&labels_reg_key])?;
            let label_names = Self::extract_string_array(&labels_result);

            let reltypes_reg_key = self.kp(key_reltype_names(name));
            let reltypes_result = self.call_cmd(ctx, "SMEMBERS", &[&reltypes_reg_key])?;
            let reltype_names = Self::extract_string_array(&reltypes_result);

            // Delete per-node keys.
            for id_str in &node_ids {
                if let Ok(id) = id_str.parse::<u64>() {
                    let nk = self.kp(key_node(name, id));
                    self.call_cmd(ctx, "DEL", &[&nk])?;
                    let ok = self.kp(key_out(name, id));
                    self.call_cmd(ctx, "DEL", &[&ok])?;
                    let ik = self.kp(key_in(name, id));
                    self.call_cmd(ctx, "DEL", &[&ik])?;
                }
            }

            // Delete per-edge keys.
            for id_str in &edge_ids {
                if let Ok(id) = id_str.parse::<u64>() {
                    let ek = self.kp(key_edge(name, id));
                    self.call_cmd(ctx, "DEL", &[&ek])?;
                }
            }

            // Delete label index sets.
            for label in &label_names {
                let lk = self.kp(key_label(name, label));
                self.call_cmd(ctx, "DEL", &[&lk])?;
            }

            // Delete reltype index sets.
            for reltype in &reltype_names {
                let rk = self.kp(key_reltype(name, reltype));
                self.call_cmd(ctx, "DEL", &[&rk])?;
            }

            // Delete fixed keys.
            for key in [
                self.kp(key_node_seq(name)),
                self.kp(key_edge_seq(name)),
                self.kp(key_nodes_set(name)),
                self.kp(key_edges_set(name)),
                self.kp(key_indexes(name)),
                self.kp(key_label_names(name)),
                self.kp(key_reltype_names(name)),
            ] {
                self.call_cmd(ctx, "DEL", &[&key])?;
            }

            // Remove from registry and clear deleting marker.
            self.call_cmd(ctx, "SREM", &[&graphs_key, name])?;
            self.call_cmd(ctx, "DEL", &[&deleting_key])?;

            Ok(())
        })
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        self.with_ctx(|ctx| {
            let graphs_key = self.kp(KEY_GRAPHS.to_string());
            let result = self.call_cmd(ctx, "SISMEMBER", &[&graphs_key, name])?;
            let is_member = Self::expect_integer(&result, "SISMEMBER")?;
            if is_member == 0 {
                return Ok(false);
            }
            // Check if graph is being deleted.
            let deleting_key = self.kp(key_graph_deleting(name));
            let exists = self.call_cmd(ctx, "EXISTS", &[&deleting_key])?;
            Ok(Self::expect_integer(&exists, "EXISTS")? == 0)
        })
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        self.with_ctx(|ctx| {
            let graphs_key = self.kp(KEY_GRAPHS.to_string());
            let result = self.call_cmd(ctx, "SMEMBERS", &[&graphs_key])?;
            let mut names = Vec::new();
            for name in Self::extract_string_array(&result) {
                // Skip graphs that are being deleted.
                let deleting_key = self.kp(key_graph_deleting(&name));
                let exists = self.call_cmd(ctx, "EXISTS", &[&deleting_key])?;
                if Self::expect_integer(&exists, "EXISTS")? == 0 {
                    names.push(name);
                }
            }
            names.sort();
            Ok(names)
        })
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        self.with_ctx(|ctx| {
            let nodes_key = self.kp(key_nodes_set(graph));
            let node_count_result = self.call_cmd(ctx, "SCARD", &[&nodes_key])?;
            let node_count = Self::expect_integer(&node_count_result, "SCARD")? as u64;

            let edges_key = self.kp(key_edges_set(graph));
            let edge_count_result = self.call_cmd(ctx, "SCARD", &[&edges_key])?;
            let edge_count = Self::expect_integer(&edge_count_result, "SCARD")? as u64;

            // Get label count from registry.
            let labels_reg_key = self.kp(key_label_names(graph));
            let labels_result = self.call_cmd(ctx, "SMEMBERS", &[&labels_reg_key])?;
            let label_names = Self::extract_string_array(&labels_result);
            // Count only labels that have at least one member.
            let mut active_labels = 0u32;
            for label in &label_names {
                let label_key = self.kp(key_label(graph, label));
                let card_result = self.call_cmd(ctx, "SCARD", &[&label_key])?;
                if Self::expect_integer(&card_result, "SCARD")? > 0 {
                    active_labels += 1;
                }
            }

            // Get reltype count from registry.
            let reltypes_reg_key = self.kp(key_reltype_names(graph));
            let reltypes_result = self.call_cmd(ctx, "SMEMBERS", &[&reltypes_reg_key])?;
            let reltype_names = Self::extract_string_array(&reltypes_result);
            let mut active_reltypes = 0u32;
            for reltype in &reltype_names {
                let reltype_key = self.kp(key_reltype(graph, reltype));
                let card_result = self.call_cmd(ctx, "SCARD", &[&reltype_key])?;
                if Self::expect_integer(&card_result, "SCARD")? > 0 {
                    active_reltypes += 1;
                }
            }

            Ok(GraphStats {
                node_count,
                edge_count,
                label_count: active_labels,
                rel_type_count: active_reltypes,
            })
        })
    }

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        self.with_ctx(|ctx| {
            let registry_key = self.kp(key_label_names(graph));
            let result = self.call_cmd(ctx, "SMEMBERS", &[&registry_key])?;
            let label_names = Self::extract_string_array(&result);

            // Filter to labels that have at least one member (active labels).
            let mut active = Vec::new();
            for label in label_names {
                let label_key = self.kp(key_label(graph, &label));
                let card_result = self.call_cmd(ctx, "SCARD", &[&label_key])?;
                if Self::expect_integer(&card_result, "SCARD")? > 0 {
                    active.push(label);
                }
            }
            active.sort_unstable();
            Ok(active)
        })
    }

    // ── Transaction support — not supported ─────────────────────────────────

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn tx_snapshot(&self, _graphs: &[&str]) -> Result<Option<Box<dyn GraphStore>>, StorageError> {
        Ok(None)
    }

    fn tx_commit(&self, _snapshot: &dyn GraphStore) -> Result<(), StorageError> {
        Ok(())
    }
}

// ── Simple range matching for find_nodes_by_label_and_property ──────────────
//
// Duplicated from sd2-redis-store/src/range.rs since we cannot depend on that
// crate without pulling in the entire Redis TCP client.

fn range_matches(val: &Value, range: &RangeQuery<'_>) -> bool {
    match range {
        RangeQuery::Eq(expected) => val == *expected,
        RangeQuery::Lt(bound) => partial_cmp_values(val, bound).is_some_and(|o| o.is_lt()),
        RangeQuery::Lte(bound) => partial_cmp_values(val, bound).is_some_and(|o| !o.is_gt()),
        RangeQuery::Gt(bound) => partial_cmp_values(val, bound).is_some_and(|o| o.is_gt()),
        RangeQuery::Gte(bound) => partial_cmp_values(val, bound).is_some_and(|o| !o.is_lt()),
        RangeQuery::Between { lo, hi, inclusive } => {
            let ge_lo = partial_cmp_values(val, lo).is_some_and(|o| !o.is_lt());
            let le_hi = if *inclusive {
                partial_cmp_values(val, hi).is_some_and(|o| !o.is_gt())
            } else {
                partial_cmp_values(val, hi).is_some_and(|o| o.is_lt())
            };
            ge_lo && le_hi
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

fn partial_cmp_values(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Int(a), Value::Int(b)) => a.partial_cmp(b),
        (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
        (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b),
        (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)),
        (Value::Str(a), Value::Str(b)) => a.partial_cmp(b),
        (Value::Bool(a), Value::Bool(b)) => a.partial_cmp(b),
        _ => None,
    }
}

// Implement Send + Sync: ModuleRedisStore holds no data that crosses threads.
// The thread-local context is per-thread by definition. The key_prefix is just
// a String. The struct itself is stored in a global OnceLock but only accessed
// from the module's command handler thread.
unsafe impl Send for ModuleRedisStore {}
unsafe impl Sync for ModuleRedisStore {}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Key schema compatibility tests ──────────────────────────────────────
    // Verify that the key functions here produce identical output to
    // sd2-redis-store/src/keys.rs.

    #[test]
    fn key_node_format() {
        assert_eq!(key_node("mydb", 42), "G:{mydb}:node:42");
        assert_eq!(key_node("test", 0), "G:{test}:node:0");
    }

    #[test]
    fn key_edge_format() {
        assert_eq!(key_edge("mydb", 99), "G:{mydb}:edge:99");
    }

    #[test]
    fn key_node_seq_format() {
        assert_eq!(key_node_seq("g1"), "G:{g1}:node_seq");
    }

    #[test]
    fn key_edge_seq_format() {
        assert_eq!(key_edge_seq("g1"), "G:{g1}:edge_seq");
    }

    #[test]
    fn key_label_format() {
        assert_eq!(key_label("mydb", "Person"), "G:{mydb}:label:Person");
    }

    #[test]
    fn key_reltype_format() {
        assert_eq!(key_reltype("mydb", "KNOWS"), "G:{mydb}:reltype:KNOWS");
    }

    #[test]
    fn key_out_format() {
        assert_eq!(key_out("mydb", 7), "G:{mydb}:out:7");
    }

    #[test]
    fn key_in_format() {
        assert_eq!(key_in("mydb", 7), "G:{mydb}:in:7");
    }

    #[test]
    fn key_nodes_set_format() {
        assert_eq!(key_nodes_set("g"), "G:{g}:nodes");
    }

    #[test]
    fn key_edges_set_format() {
        assert_eq!(key_edges_set("g"), "G:{g}:edges");
    }

    #[test]
    fn key_indexes_format() {
        assert_eq!(key_indexes("db"), "G:{db}:indexes");
    }

    #[test]
    fn key_graph_deleting_format() {
        assert_eq!(key_graph_deleting("db"), "G:{db}:_deleting");
    }

    #[test]
    fn key_label_names_format() {
        assert_eq!(key_label_names("db"), "G:{db}:_labels");
    }

    #[test]
    fn key_reltype_names_format() {
        assert_eq!(key_reltype_names("db"), "G:{db}:_reltypes");
    }

    #[test]
    fn key_graphs_constant() {
        assert_eq!(KEY_GRAPHS, "__sd2_registry:graphs");
        assert!(!KEY_GRAPHS.starts_with("G:"));
    }

    #[test]
    fn all_graph_keys_contain_hash_tag() {
        let g = "testgraph";
        let tag = format!("{{{g}}}");
        assert!(key_node(g, 1).contains(&tag));
        assert!(key_edge(g, 1).contains(&tag));
        assert!(key_node_seq(g).contains(&tag));
        assert!(key_edge_seq(g).contains(&tag));
        assert!(key_label(g, "L").contains(&tag));
        assert!(key_reltype(g, "R").contains(&tag));
        assert!(key_out(g, 1).contains(&tag));
        assert!(key_in(g, 1).contains(&tag));
        assert!(key_nodes_set(g).contains(&tag));
        assert!(key_edges_set(g).contains(&tag));
        assert!(key_indexes(g).contains(&tag));
        assert!(key_graph_deleting(g).contains(&tag));
        assert!(key_label_names(g).contains(&tag));
        assert!(key_reltype_names(g).contains(&tag));
    }

    // ── Adjacency encoding ─────────────────────────────────────────────────

    #[test]
    fn encode_decode_adj_round_trip() {
        let encoded = encode_adj(EdgeId(10), NodeId(20), "KNOWS");
        assert_eq!(encoded, "10:20:KNOWS");
        let decoded = decode_adj(&encoded).expect("should decode");
        assert_eq!(decoded.edge_id, EdgeId(10));
        assert_eq!(decoded.neighbor_id, NodeId(20));
        assert_eq!(decoded.rel_type, "KNOWS");
    }

    #[test]
    fn decode_adj_with_colons_in_rel_type() {
        let encoded = "5:10:HAS:COLON:IN:NAME";
        let decoded = decode_adj(encoded).unwrap();
        assert_eq!(decoded.edge_id, EdgeId(5));
        assert_eq!(decoded.neighbor_id, NodeId(10));
        assert_eq!(decoded.rel_type, "HAS:COLON:IN:NAME");
    }

    #[test]
    fn decode_adj_malformed_returns_none() {
        assert!(decode_adj("").is_none());
        assert!(decode_adj("single").is_none());
        assert!(decode_adj("one:two").is_none());
        assert!(decode_adj("notanumber:20:REL").is_none());
    }

    // ── Range matching ──────────────────────────────────────────────────────

    #[test]
    fn range_eq_match() {
        assert!(range_matches(
            &Value::Int(42),
            &RangeQuery::Eq(&Value::Int(42))
        ));
        assert!(!range_matches(
            &Value::Int(42),
            &RangeQuery::Eq(&Value::Int(43))
        ));
    }

    #[test]
    fn range_starts_with() {
        assert!(range_matches(
            &Value::Str("hello world".into()),
            &RangeQuery::StartsWith("hello")
        ));
        assert!(!range_matches(
            &Value::Str("goodbye".into()),
            &RangeQuery::StartsWith("hello")
        ));
    }

    // ── JSON serialization ──────────────────────────────────────────────────

    #[test]
    fn props_json_round_trip() {
        let mut props = IndexMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        props.insert("age".to_string(), Value::Int(30));
        let json = props_to_json(&props).unwrap();
        let parsed = json_to_props(&json).unwrap();
        assert_eq!(parsed.get("name"), Some(&Value::Str("Alice".into())));
        assert_eq!(parsed.get("age"), Some(&Value::Int(30)));
    }

    #[test]
    fn labels_json_round_trip() {
        let labels = vec!["Person".to_string(), "Employee".to_string()];
        let json = serde_json::to_string(&labels).unwrap();
        let parsed: Vec<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, labels);
    }
}
