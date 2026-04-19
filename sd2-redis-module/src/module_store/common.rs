//! Shared helpers used across `ModuleRedisStore` operations:
//! - thread-local context plumbing (`with_ctx`)
//! - Redis command invocation via the module API (`call_cmd`)
//! - reply decoders (`expect_integer`, `extract_string_array`, etc.)
//! - HMGET → `NodeData`/`EdgeData` parsers

use redis_module::{Context, RedisValue};

use cypher_core::error::StorageError;
use cypher_core::serde_value::json_to_props;
use cypher_core::storage::{AdjEntry, EdgeData, EdgeId, NodeData, NodeId};

use super::keys::{decode_adj, key_indexes};
use super::{ModuleRedisStore, MODULE_CTX};

impl ModuleRedisStore {
    /// Execute a closure with access to the current module context.
    pub(super) fn with_ctx<F, R>(&self, f: F) -> Result<R, StorageError>
    where
        F: FnOnce(&Context) -> Result<R, StorageError>,
    {
        MODULE_CTX.with(|cell| {
            let ptr = cell.borrow();
            let ctx_ptr = ptr.ok_or(StorageError::Backend {
                msg: "ModuleRedisStore: no module context set".into(),
            })?;
            // SAFETY: `ctx_ptr` is a `*const Context` previously stored by
            // `set_context()` at the start of a command callback. Valkey
            // module commands are dispatched on a single thread, so there is
            // no concurrent access. The pointer remains valid for the full
            // duration of the callback; misuse (forgetting to call
            // `clear_context()` at the end of the callback before the
            // `Context` borrow ends) would be use-after-free, which is why
            // this pair must always be invoked symmetrically.
            let ctx = unsafe { &*ctx_ptr };
            f(ctx)
        })
    }

    /// Apply the configured key prefix to a key. No-op when prefix is empty.
    #[inline]
    pub(super) fn kp(&self, key: String) -> String {
        if self.key_prefix.is_empty() {
            key
        } else {
            format!("{}{}", self.key_prefix, key)
        }
    }

    // ── ctx.call() helpers ──────────────────────────────────────────────────

    /// Execute a Redis command via the module API and return the RedisValue.
    pub(super) fn call_cmd(
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
    pub(super) fn expect_integer(val: &RedisValue, cmd: &str) -> Result<i64, StorageError> {
        match val {
            RedisValue::Integer(i) => Ok(*i),
            other => Err(StorageError::Backend {
                msg: format!("expected integer from {cmd}, got: {other:?}"),
            }),
        }
    }

    /// Extract a bulk string from a RedisValue reply, returning None for Null.
    pub(super) fn maybe_bulk_string(val: &RedisValue) -> Option<String> {
        match val {
            RedisValue::BulkString(s) => Some(s.clone()),
            RedisValue::SimpleString(s) => Some(s.clone()),
            RedisValue::SimpleStringStatic(s) => Some(s.to_string()),
            _ => None,
        }
    }

    /// Extract an array of strings from a RedisValue::Array reply (SMEMBERS result).
    pub(super) fn extract_string_array(val: &RedisValue) -> Vec<String> {
        match val {
            RedisValue::Array(arr) => arr.iter().filter_map(Self::maybe_bulk_string).collect(),
            _ => Vec::new(),
        }
    }

    /// Extract fields from an HMGET-style array reply.
    /// Returns a Vec<Option<String>> matching the requested fields.
    pub(super) fn extract_hmget_fields(val: &RedisValue) -> Vec<Option<String>> {
        match val {
            RedisValue::Array(arr) => arr.iter().map(Self::maybe_bulk_string).collect(),
            _ => Vec::new(),
        }
    }

    /// Return the set of active property indexes for a graph.
    pub(super) fn active_indexes(
        &self,
        graph: &str,
    ) -> Result<std::collections::HashSet<String>, StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_indexes(graph));
            let result = self.call_cmd(ctx, "SMEMBERS", &[&key])?;
            Ok(Self::extract_string_array(&result).into_iter().collect())
        })
    }

    // ── Node/edge parsing from Redis replies ─────────────────────────────────

    pub(super) fn parse_node_from_hmget(
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

    pub(super) fn parse_edge_from_hmget(
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
    pub(super) fn parse_adj_members(members: &[String]) -> Vec<AdjEntry> {
        members.iter().filter_map(|m| decode_adj(m)).collect()
    }
}
