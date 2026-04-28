//! Lua scripts for the slot-local hot-write paths (F2 in the cluster
//! consistency hardening package, issue #1769).
//!
//! Both scripts share the same fence-validation prologue: they read the
//! per-graph fence counter (`KEYS[1]`) and refuse to run if the writer's
//! expected fence (`ARGV[1]`) does not match. This closes the partial-late-
//! write window where a stalled writer whose lease was stolen could land
//! mutations after a newer writer had already begun running.
//!
//! Lua execution on a single Redis Cluster slot is atomic by definition —
//! the entire script runs without interleaving. Together with the fence
//! check this means a concurrent reader can never observe a node hash
//! without its label-index entries, or an edge hash without its reverse
//! adjacency.
//!
//! Returned shape:
//! * `STALE_FENCE` (Redis error reply) → caller maps to
//!   [`StorageError::LockFenced`].
//! * Integer (positive) → the allocated entity id.
//! * `0` → caller-supplied id was already present (collision; mapped to
//!   [`StorageError::ConstraintViolation`]).
//!
//! The scripts are intentionally narrow: they do NOT touch property
//! indexes, the graph registry, or anything else that needs cross-slot
//! co-ordination. Property indexing is layered on by the calling Rust code
//! while the writer still holds the per-graph write lock.

use cypher_core::error::StorageError;
use redis::ConnectionLike;

use crate::error_metrics::record_lock_fence_rejected;

/// Atomic node creation with labels, all-nodes membership, label-index
/// membership, and the node hash itself.
///
/// `KEYS`:
/// 1. fence key   (`G:{g}:write_lock_fence`)
/// 2. node hash   (`G:{g}:node:{id}`)
/// 3. node-seq    (`G:{g}:node_seq`)
/// 4. nodes set   (`G:{g}:nodes`)
/// 5. label-names (`G:{g}:_labels`)
///
/// `KEYS[6..N]`: label-index sets (`G:{g}:label:{label}`) in the same order
/// as the labels listed in `ARGV[6..]`.
///
/// `ARGV`:
/// 1. expected fence (decimal string)
/// 2. injected id (`""` if none — script INCRs node-seq instead)
/// 3. labels JSON
/// 4. props JSON
/// 5. number of label entries (decimal string; matches `KEYS[6..]` and the
///    label tail of `ARGV`)
///
/// `ARGV[6..N]`: label names (one per `KEYS[6..N]` entry)
///
/// Returns the new node id (positive integer) on success. Returns `0` if
/// the caller-supplied id collided with an existing node hash (caller maps
/// to `ConstraintViolation`). Returns a `STALE_FENCE` error reply if the
/// fence guard rejected the call.
pub(crate) const NODE_CREATE_SCRIPT: &str = r#"
if redis.call("GET", KEYS[1]) ~= ARGV[1] then
    return redis.error_reply("STALE_FENCE")
end

local injected = ARGV[2]
local id
if injected ~= "" then
    if redis.call("EXISTS", KEYS[2]) == 1 then
        return 0
    end
    id = tonumber(injected)
    local seq = tonumber(redis.call("GET", KEYS[3]) or "0")
    if id > seq then
        redis.call("SET", KEYS[3], tostring(id))
    end
else
    id = redis.call("INCR", KEYS[3])
    if id <= 0 then
        return redis.error_reply("ID_OVERFLOW")
    end
end

local label_count = tonumber(ARGV[5])
if label_count > 0 then
    local registry_args = {}
    for i = 1, label_count do
        registry_args[i] = ARGV[5 + i]
    end
    redis.call("SADD", KEYS[5], unpack(registry_args))
end

redis.call("SADD", KEYS[4], tostring(id))

for i = 1, label_count do
    redis.call("SADD", KEYS[5 + i], tostring(id))
end

redis.call("HSET", KEYS[2], "labels", ARGV[3], "props", ARGV[4])

return id
"#;

/// Atomic edge creation with all-edges membership, reltype-index membership,
/// out- and in-adjacency, and the edge hash itself.
///
/// `KEYS`:
/// 1. fence key       (`G:{g}:write_lock_fence`)
/// 2. edge hash       (`G:{g}:edge:{id}`)
/// 3. edge-seq        (`G:{g}:edge_seq`)
/// 4. edges set       (`G:{g}:edges`)
/// 5. reltype-names   (`G:{g}:_reltypes`)
/// 6. reltype-index   (`G:{g}:reltype:{rel_type}`)
/// 7. out-adjacency   (`G:{g}:out:{src}`)
/// 8. in-adjacency    (`G:{g}:in:{dst}`)
///
/// `ARGV`:
/// 1. expected fence
/// 2. injected id (`""` if none)
/// 3. rel_type
/// 4. src id (decimal string)
/// 5. dst id (decimal string)
/// 6. props JSON
/// 7. out-adjacency member encoding
/// 8. in-adjacency member encoding
///
/// Returns the new edge id, or `0` if a caller-supplied id collided.
pub(crate) const EDGE_CREATE_SCRIPT: &str = r#"
if redis.call("GET", KEYS[1]) ~= ARGV[1] then
    return redis.error_reply("STALE_FENCE")
end

local injected = ARGV[2]
local id
if injected ~= "" then
    if redis.call("EXISTS", KEYS[2]) == 1 then
        return 0
    end
    id = tonumber(injected)
    local seq = tonumber(redis.call("GET", KEYS[3]) or "0")
    if id > seq then
        redis.call("SET", KEYS[3], tostring(id))
    end
else
    id = redis.call("INCR", KEYS[3])
    if id <= 0 then
        return redis.error_reply("ID_OVERFLOW")
    end
end

redis.call("SADD", KEYS[5], ARGV[3])
redis.call("SADD", KEYS[4], tostring(id))
redis.call("SADD", KEYS[6], tostring(id))
redis.call("SADD", KEYS[7], ARGV[7])
redis.call("SADD", KEYS[8], ARGV[8])
redis.call(
    "HSET", KEYS[2],
    "src", ARGV[4],
    "dst", ARGV[5],
    "rel_type", ARGV[3],
    "props", ARGV[6]
)

return id
"#;

/// Atomic addition of N labels to an existing node.
///
/// `KEYS`:
/// 1. fence key       (`G:{g}:write_lock_fence`)
/// 2. node hash       (`G:{g}:node:{id}`)
/// 3. label-names     (`G:{g}:_labels`)
///
/// `KEYS[4..N]`: label-index sets (`G:{g}:label:{label}`) for the labels
///   being newly added in the same order as `ARGV[4..]`.
///
/// `ARGV`:
/// 1. expected fence (decimal string)
/// 2. node id (decimal string)
/// 3. full new labels JSON (will overwrite the `labels` field on the hash)
///
/// `ARGV[4..N]`: label names being added (one per `KEYS[4..N]` entry).
///
/// Returns 1 on success, or `redis.error_reply("NODE_MISSING")` if the
/// node hash does not exist (caller maps to `NodeNotFound`).
pub(crate) const NODE_LABELS_ADD_SCRIPT: &str = r#"
if redis.call("GET", KEYS[1]) ~= ARGV[1] then
    return redis.error_reply("STALE_FENCE")
end
if redis.call("EXISTS", KEYS[2]) ~= 1 then
    return redis.error_reply("NODE_MISSING")
end

local node_id = ARGV[2]
local added_count = #KEYS - 3
if added_count > 0 then
    local registry_args = {}
    for i = 1, added_count do
        registry_args[i] = ARGV[3 + i]
    end
    redis.call("SADD", KEYS[3], unpack(registry_args))
    for i = 1, added_count do
        redis.call("SADD", KEYS[3 + i], node_id)
    end
end

redis.call("HSET", KEYS[2], "labels", ARGV[3])
return 1
"#;

/// Atomic removal of N labels from an existing node.
///
/// `KEYS`:
/// 1. fence key       (`G:{g}:write_lock_fence`)
/// 2. node hash       (`G:{g}:node:{id}`)
///
/// `KEYS[3..N]`: label-index sets being SREM'd.
///
/// `ARGV`:
/// 1. expected fence
/// 2. node id (decimal string)
/// 3. full new labels JSON (writes back the filtered list)
///
/// Returns 1 on success, `NODE_MISSING` error reply otherwise.
pub(crate) const NODE_LABELS_REMOVE_SCRIPT: &str = r#"
if redis.call("GET", KEYS[1]) ~= ARGV[1] then
    return redis.error_reply("STALE_FENCE")
end
if redis.call("EXISTS", KEYS[2]) ~= 1 then
    return redis.error_reply("NODE_MISSING")
end

local node_id = ARGV[2]
local removed_count = #KEYS - 2
for i = 1, removed_count do
    redis.call("SREM", KEYS[2 + i], node_id)
end

redis.call("HSET", KEYS[2], "labels", ARGV[3])
return 1
"#;

/// Atomic teardown of a node: SREMs from each label index, SREMs from the
/// all-nodes set, DELs the out- and in- adjacency sets and the node hash
/// itself.
///
/// Property indexes and (for DETACH) edge teardown are handled by the
/// calling Rust code BEFORE this script runs — they require reads that
/// span keys whose names are not known up front.
///
/// `KEYS`:
/// 1. fence key       (`G:{g}:write_lock_fence`)
/// 2. node hash       (`G:{g}:node:{id}`)
/// 3. nodes set       (`G:{g}:nodes`)
/// 4. out-adjacency   (`G:{g}:out:{id}`)
/// 5. in-adjacency    (`G:{g}:in:{id}`)
///
/// `KEYS[6..N]`: label-index sets to SREM the node id from.
///
/// `ARGV`:
/// 1. expected fence
/// 2. node id (decimal string)
///
/// Returns 1 on success.
pub(crate) const NODE_DELETE_SCRIPT: &str = r#"
if redis.call("GET", KEYS[1]) ~= ARGV[1] then
    return redis.error_reply("STALE_FENCE")
end

local node_id = ARGV[2]
local label_count = #KEYS - 5
for i = 1, label_count do
    redis.call("SREM", KEYS[5 + i], node_id)
end

redis.call("SREM", KEYS[3], node_id)
redis.call("DEL", KEYS[4])
redis.call("DEL", KEYS[5])
redis.call("DEL", KEYS[2])
return 1
"#;

/// Atomic teardown of an edge: SREMs from reltype index, all-edges set,
/// out- and in-adjacency, then DELs the edge hash.
///
/// `KEYS`:
/// 1. fence key       (`G:{g}:write_lock_fence`)
/// 2. edge hash       (`G:{g}:edge:{id}`)
/// 3. edges set       (`G:{g}:edges`)
/// 4. reltype-index   (`G:{g}:reltype:{rel_type}`)
/// 5. out-adjacency   (`G:{g}:out:{src}`)
/// 6. in-adjacency    (`G:{g}:in:{dst}`)
///
/// `ARGV`:
/// 1. expected fence
/// 2. edge id (decimal string)
/// 3. out-adjacency member encoding
/// 4. in-adjacency member encoding
///
/// Returns 1 on success.
pub(crate) const EDGE_DELETE_SCRIPT: &str = r#"
if redis.call("GET", KEYS[1]) ~= ARGV[1] then
    return redis.error_reply("STALE_FENCE")
end

local edge_id = ARGV[2]
redis.call("SREM", KEYS[4], edge_id)
redis.call("SREM", KEYS[3], edge_id)
redis.call("SREM", KEYS[5], ARGV[3])
redis.call("SREM", KEYS[6], ARGV[4])
redis.call("DEL", KEYS[2])
return 1
"#;

/// Map a Lua hot-write script error into the appropriate `StorageError`.
///
/// Recognises the two error replies emitted by the scripts above:
/// * `STALE_FENCE` → [`StorageError::LockFenced`] + fence-rejection metric.
/// * `ID_OVERFLOW` → [`StorageError::IdOverflow`].
///
/// All other backend errors fall through to [`crate::backend_storage_error`].
pub(crate) fn map_script_error(graph: &str, err: redis::RedisError) -> StorageError {
    let msg = err.to_string();
    if msg.contains("STALE_FENCE") {
        record_lock_fence_rejected(graph);
        return StorageError::LockFenced {
            graph: graph.to_string(),
        };
    }
    if msg.contains("ID_OVERFLOW") {
        return StorageError::IdOverflow {
            graph: graph.to_string(),
        };
    }
    crate::backend_storage_error(err)
}

/// Run a `redis::Script` against a `ConnectionLike` and map errors via
/// [`map_script_error`].
pub(crate) fn run_hot_write_script<T>(
    script: &redis::Script,
    conn: &mut dyn ConnectionLike,
    graph: &str,
    keys: &[String],
    args: &[String],
) -> Result<T, StorageError>
where
    T: redis::FromRedisValue,
{
    let mut invocation = script.prepare_invoke();
    for key in keys {
        invocation.key(key.as_str());
    }
    for arg in args {
        invocation.arg(arg.as_str());
    }
    invocation
        .invoke::<T>(conn)
        .map_err(|e| map_script_error(graph, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use redis::{ErrorKind, RedisError};

    #[test]
    fn stale_fence_error_maps_to_lock_fenced_with_graph_context() {
        let err = RedisError::from((ErrorKind::Extension, "STALE_FENCE"));
        let mapped = map_script_error("g1", err);
        assert!(matches!(mapped, StorageError::LockFenced { ref graph } if graph == "g1"));
    }

    #[test]
    fn id_overflow_error_maps_to_id_overflow() {
        let err = RedisError::from((ErrorKind::Extension, "ID_OVERFLOW"));
        let mapped = map_script_error("g2", err);
        assert!(matches!(mapped, StorageError::IdOverflow { ref graph } if graph == "g2"));
    }

    #[test]
    fn unrelated_io_error_falls_through_to_backend() {
        let err = RedisError::from((ErrorKind::Io, "kaboom"));
        let mapped = map_script_error("g3", err);
        assert!(matches!(mapped, StorageError::Backend { .. }));
    }
}
