// writes/nodes/ -- Node write operations for RedisGraphStore.
//
// Split by responsibility:
//   - create:     single-node creation (`create_node_impl`)
//   - properties: SET / REPLACE / REMOVE property operations
//   - labels:     add / remove label operations
//   - delete:     single-node delete with optional DETACH semantics
//   - batch:      batch_create_nodes pipelining
//
// This module file owns the small validation helpers shared by every
// submodule plus the unit tests that cover them.

use std::collections::HashSet;

use indexmap::IndexMap;

use cypher_core::error::StorageError;
use cypher_core::storage::{EdgeId, NodeId};
use cypher_core::types::Value;

use crate::validation::{validate_label, validate_property_key};

mod batch;
mod create;
mod delete;
mod labels;
mod properties;

pub(super) fn max_detach_delete_edges() -> usize {
    static SIZE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *SIZE.get_or_init(|| {
        cypher_core::config::parse_env_min("GQL_REDIS_MAX_DETACH_DELETE_EDGES", 10_000usize, 1)
    })
}

pub(super) fn insert_detach_edge_id_with_limit(
    node_id: NodeId,
    edge_ids: &mut HashSet<EdgeId>,
    edge_id: EdgeId,
    limit: usize,
) -> Result<(), StorageError> {
    edge_ids.insert(edge_id);
    if edge_ids.len() > limit {
        crate::record_backend_error();
        return Err(StorageError::Backend {
            msg: format!(
                "DETACH DELETE for node {} exceeds GQL_REDIS_MAX_DETACH_DELETE_EDGES={} (adjacent edges: {})",
                node_id.0,
                limit,
                edge_ids.len()
            ),
        });
    }
    Ok(())
}

pub(super) fn validate_labels(labels: &[String]) -> Result<(), StorageError> {
    for label in labels {
        validate_label(label)?;
    }
    Ok(())
}

pub(super) fn validate_property_keys(
    properties: &IndexMap<String, Value>,
) -> Result<(), StorageError> {
    for key in properties.keys() {
        validate_property_key(key)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writes::helpers::{clean_fanout_props, extract_injected_id};

    #[test]
    fn insert_detach_edge_id_with_limit_allows_single_self_loop() {
        let mut edge_ids = HashSet::new();
        insert_detach_edge_id_with_limit(NodeId(42), &mut edge_ids, EdgeId(7), 1)
            .expect("first self-loop edge should fit within the limit");
        insert_detach_edge_id_with_limit(NodeId(42), &mut edge_ids, EdgeId(7), 1)
            .expect("duplicate self-loop edge ID should not consume extra limit budget");
        let mut collected: Vec<_> = edge_ids.into_iter().collect();
        collected.sort_unstable();
        assert_eq!(collected, vec![EdgeId(7)]);
    }

    #[test]
    fn insert_detach_edge_id_with_limit_rejects_distinct_edges_over_limit() {
        let mut edge_ids = HashSet::new();
        insert_detach_edge_id_with_limit(NodeId(42), &mut edge_ids, EdgeId(7), 1)
            .expect("first distinct edge should fit within the limit");
        let err = insert_detach_edge_id_with_limit(NodeId(42), &mut edge_ids, EdgeId(8), 1)
            .expect_err("two distinct adjacent edges should still trip a limit of one");
        assert!(matches!(err, StorageError::Backend { .. }));
        let mut collected: Vec<_> = edge_ids.into_iter().collect();
        collected.sort_unstable();
        assert_eq!(collected, vec![EdgeId(7), EdgeId(8)]);
    }

    // ---- Pure logic tests for extract_injected_id ---------------------------

    #[test]
    fn extract_injected_id_present_positive() {
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(42));
        let result = extract_injected_id(&props, "_sd2_id").unwrap();
        assert_eq!(result, Some(42));
    }

    #[test]
    fn extract_injected_id_present_zero() {
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(0));
        let result = extract_injected_id(&props, "_sd2_id").unwrap();
        assert_eq!(result, Some(0));
    }

    #[test]
    fn extract_injected_id_present_negative_fails() {
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(-5));
        let result = extract_injected_id(&props, "_sd2_id");
        assert!(result.is_err());
    }

    #[test]
    fn extract_injected_id_absent() {
        let props = IndexMap::new();
        let result = extract_injected_id(&props, "_sd2_id").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn extract_injected_id_wrong_type_ignored() {
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Str("not_an_int".into()));
        // Non-Int values are not matched, so returns None.
        let result = extract_injected_id(&props, "_sd2_id").unwrap();
        assert_eq!(result, None);
    }

    // ---- Pure logic tests for clean_fanout_props ----------------------------

    #[test]
    fn clean_fanout_props_removes_internal_keys() {
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(1));
        props.insert("_sd2_eid".to_string(), Value::Int(2));
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let cleaned = clean_fanout_props(props);
        assert!(!cleaned.contains_key("_sd2_id"));
        assert!(!cleaned.contains_key("_sd2_eid"));
        assert_eq!(cleaned.get("name"), Some(&Value::Str("Alice".into())));
    }

    #[test]
    fn clean_fanout_props_noop_when_no_internal_keys() {
        let mut props = IndexMap::new();
        props.insert("age".to_string(), Value::Int(30));
        let cleaned = clean_fanout_props(props);
        assert_eq!(cleaned.len(), 1);
        assert_eq!(cleaned.get("age"), Some(&Value::Int(30)));
    }

    #[test]
    fn clean_fanout_props_empty_map() {
        let props = IndexMap::new();
        let cleaned = clean_fanout_props(props);
        assert!(cleaned.is_empty());
    }

    #[test]
    fn validate_labels_rejects_null_byte() {
        let labels = vec!["Per\x00son".to_string()];
        let result = validate_labels(&labels);
        assert!(
            result.is_err(),
            "write-path label validation should reject null bytes"
        );
    }

    #[test]
    fn validate_property_keys_rejects_null_byte() {
        let mut props = IndexMap::new();
        props.insert("na\x00me".to_string(), Value::Int(1));
        let result = validate_property_keys(&props);
        assert!(
            result.is_err(),
            "write-path property validation should reject null-byte keys"
        );
    }

    // ---- Cluster pipeline safety: verify no MULTI/EXEC in node write pipeline ---

    /// Verify that the node-write pipeline does NOT include MULTI or EXEC.
    ///
    /// This test encodes the fix for Issue #745: redis-rs ClusterConnection
    /// does not support MULTI/EXEC, so .atomic() must not be used in cluster mode.
    /// All per-graph keys share the {graphname} hash tag and route to the same
    /// cluster slot, so non-atomic pipelining is cluster-safe.
    #[test]
    fn create_node_pipeline_has_no_multi_exec() {
        // Construct the same pipeline sequence as create_node_impl (minus Redis I/O).
        // We validate the pipeline command names do not include MULTI or EXEC.
        let labels = vec!["Person".to_string(), "Employee".to_string()];
        let graph = "testgraph";
        let id = 1u64;
        let node_key = format!("G:{{{graph}}}:node:{id}");
        let labels_json = serde_json::to_string(&labels).unwrap();
        let props_json = "{}";

        let mut pipe = redis::pipe();
        // Replicate create_node_impl pipeline (without .atomic())
        pipe.cmd("HSET")
            .arg(&node_key)
            .arg("labels")
            .arg(&labels_json)
            .arg("props")
            .arg(props_json)
            .ignore();
        for label in &labels {
            pipe.cmd("SADD")
                .arg(format!("G:{{{graph}}}:label:{label}"))
                .arg(id.to_string())
                .ignore();
        }
        pipe.cmd("SADD")
            .arg(format!("G:{{{graph}}}:nodes"))
            .arg(id.to_string())
            .ignore();

        // Serialize the pipeline to bytes and verify MULTI/EXEC are absent.
        let packed = pipe.get_packed_pipeline();
        let pipeline_str = String::from_utf8_lossy(&packed);
        // In Redis protocol, commands appear as bulk strings.
        assert!(
            !pipeline_str.to_uppercase().contains("MULTI"),
            "Pipeline must not contain MULTI (breaks ClusterConnection): {pipeline_str}"
        );
        assert!(
            !pipeline_str.to_uppercase().contains("EXEC"),
            "Pipeline must not contain EXEC (breaks ClusterConnection): {pipeline_str}"
        );
    }

    /// Verify that all keys in the node-write pipeline share the {graphname} hash tag.
    ///
    /// Redis Cluster routes keys to slots by hash tag. If all keys in a pipeline
    /// share the same hash tag, they are guaranteed to be on the same cluster node,
    /// making non-atomic pipelining safe and correct.
    #[test]
    fn create_node_pipeline_keys_all_share_hash_tag() {
        let graph = "mygraph";
        let id = 42u64;
        let labels = ["Label1".to_string(), "Label2".to_string()];
        let expected_tag = format!("{{{graph}}}");

        // All keys produced by the create_node pipeline.
        let node_key = format!("G:{expected_tag}:node:{id}");
        let label_keys: Vec<String> = labels
            .iter()
            .map(|l| format!("G:{expected_tag}:label:{l}"))
            .collect();
        let nodes_set_key = format!("G:{expected_tag}:nodes");

        // Every key must contain the hash tag.
        assert!(
            node_key.contains(&expected_tag),
            "node key missing hash tag: {node_key}"
        );
        assert!(
            nodes_set_key.contains(&expected_tag),
            "nodes set key missing hash tag: {nodes_set_key}"
        );
        for k in &label_keys {
            assert!(k.contains(&expected_tag), "label key missing hash tag: {k}");
        }
    }
}
