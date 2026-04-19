// sd2-redis-store: Redis/Valkey-backed implementation of the GraphStore trait.
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
//   Delete marker:   G:{g}:_deleting        — SET while delete_graph cleanup is in progress
//   Propidx manifest:G:{g}:_propidx         — SADD fully-qualified propidx keys for delete_graph
//   Graph registry:  __sd2_registry:graphs  — SADD graph name

pub mod create_store;
pub mod fanout;
pub(crate) mod lock;
pub mod redis_memory_metrics;

mod audit;
mod connection;
mod entity_reads;
mod error_metrics;
mod graph_store_impl;
mod indexing;
mod keys;
mod locked_conn;
mod range;
mod reads;
mod store;
mod validation;
mod writes;

pub use audit::GraphMetadataAuditReport;
pub use connection::PoolStats;
pub(crate) use error_metrics::backend_storage_error;
pub use error_metrics::{backend_error_count, backend_error_msg, record_backend_error};
pub use fanout::FanoutStore;
pub use store::RedisGraphStore;

pub(crate) use locked_conn::LockedRedisConn;

/// Upper bound on nodes a graph may contain before `delete_graph` is refused.
///
/// Deletion enumerates the all-nodes and all-edges sets via SMEMBERS, then
/// issues explicit DEL commands — safe on Redis Cluster because every key
/// for the same graph hashes to the same slot.  Graphs larger than this
/// limit must be deleted in parts or via an offline tool.
pub const MAX_NODES_FOR_DELETE: usize = 100_000;

#[cfg(test)]
mod key_schema_tests {
    use crate::keys;

    #[test]
    fn key_graphs_constant_value() {
        assert_eq!(keys::KEY_GRAPHS, "__sd2_registry:graphs");
    }

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
}
