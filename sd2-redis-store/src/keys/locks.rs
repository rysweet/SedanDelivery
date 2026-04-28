// Distributed write-lock and registry keys.

/// Distributed write-lock key for a graph.  Used with CAS-release Lua to
/// coordinate concurrent write operations on the same graph.
pub(crate) fn key_graph_lock(g: &str) -> String {
    format!("G:{{{g}}}:write_lock")
}

/// Monotonic per-graph fence counter for the graph write lock.  Bumped every
/// time the graph lock is acquired (including after TTL-driven reacquire).
/// Stored next to the lock key (same `{g}` hash tag → same cluster slot) so
/// acquire + INCR can be performed in a single Lua script.
///
/// Hot-write Lua scripts validate the caller's expected fence against this
/// key as their first action, so a stalled writer whose lease has been stolen
/// fails closed (cf. ADR-005, `docs/cluster-write-safety.md`).
pub(crate) fn key_graph_lock_fence(g: &str) -> String {
    format!("G:{{{g}}}:write_lock_fence")
}

/// Distributed write-lock key for mutating a single existing node.
pub(crate) fn key_node_write_lock(g: &str, id: u64) -> String {
    format!("G:{{{g}}}:write_lock:node:{id}")
}

/// Distributed write-lock key for mutating a single existing edge by ID.
pub(crate) fn key_edge_id_write_lock(g: &str, id: u64) -> String {
    format!("G:{{{g}}}:write_lock:edge:{id}")
}

/// Distributed write-lock key for creating a relationship between two nodes.
pub(crate) fn key_edge_write_lock(g: &str, src: u64, dst: u64, rel_type: &str) -> String {
    format!("G:{{{g}}}:write_lock:edge:{src}:{dst}:{rel_type}")
}

/// Distributed coordination key for batch-creating nodes. Used alongside the
/// graph write lock so batch node operations can be identified separately while
/// remaining in the same graph hash slot.
pub(crate) fn key_batch_node_write_lock(g: &str) -> String {
    format!("G:{{{g}}}:write_lock:batch_nodes")
}

/// Distributed coordination key for batch-creating edges. Used alongside the
/// graph write lock so batch edge operations can be identified separately while
/// remaining in the same graph hash slot.
pub(crate) fn key_batch_edge_write_lock(g: &str) -> String {
    format!("G:{{{g}}}:write_lock:batch_edges")
}

/// Registry key for graph names.  Uses a `__sd2_registry:` prefix that cannot
/// collide with per-graph data keys (which all start with `G:`).  The key has
/// no Redis Cluster hash tag, so it routes to its own slot; this is fine because
/// it's only accessed by standalone operations (SADD, SISMEMBER, SMEMBERS, SREM).
pub(crate) const KEY_GRAPHS: &str = "__sd2_registry:graphs";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_graphs_constant() {
        assert_eq!(KEY_GRAPHS, "__sd2_registry:graphs");
        // Must NOT start with "G:" to avoid collisions with per-graph data.
        assert!(!KEY_GRAPHS.starts_with("G:"));
    }

    #[test]
    fn key_node_write_lock_format() {
        assert_eq!(key_node_write_lock("db", 7), "G:{db}:write_lock:node:7");
    }

    #[test]
    fn key_edge_id_write_lock_format() {
        assert_eq!(
            key_edge_id_write_lock("db", 11),
            "G:{db}:write_lock:edge:11"
        );
    }

    #[test]
    fn key_edge_write_lock_format() {
        assert_eq!(
            key_edge_write_lock("db", 1, 2, "KNOWS"),
            "G:{db}:write_lock:edge:1:2:KNOWS"
        );
    }

    #[test]
    fn key_batch_node_write_lock_format() {
        assert_eq!(
            key_batch_node_write_lock("db"),
            "G:{db}:write_lock:batch_nodes"
        );
    }

    #[test]
    fn key_batch_edge_write_lock_format() {
        assert_eq!(
            key_batch_edge_write_lock("db"),
            "G:{db}:write_lock:batch_edges"
        );
    }
}
