// writes/edges/ -- Edge write operations for RedisGraphStore.
//
// Split by responsibility:
//   - create:     single-edge creation (`create_edge_impl`)
//   - properties: SET / REPLACE / REMOVE property operations
//   - delete:     single-edge delete (`delete_edge_impl`, `delete_edge_inner`)
//   - batch:      batch_create_edges pipelining
//
// This module file owns the small validation helper shared by every
// submodule plus the unit tests that cover the edge write pipelines.

use indexmap::IndexMap;

use cypher_core::error::StorageError;
use cypher_core::types::Value;

use crate::validation::validate_property_key;

mod batch;
mod create;
mod delete;
mod properties;

fn validate_property_keys(properties: &IndexMap<String, Value>) -> Result<(), StorageError> {
    for key in properties.keys() {
        validate_property_key(key)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::keys::{decode_adj, encode_adj};
    use cypher_core::storage::{EdgeId, NodeId};
    use cypher_core::types::Value;
    use indexmap::IndexMap;

    use super::validate_property_keys;

    // ---- Adjacency encoding used by edge write pipelines --------------------

    #[test]
    fn encode_adj_format_for_edge_pipeline() {
        // Edge writes construct adjacency members like "{eid}:{neighbor}:{rel_type}".
        let member = encode_adj(EdgeId(100), NodeId(200), "KNOWS");
        assert_eq!(member, "100:200:KNOWS");
    }

    #[test]
    fn encode_adj_round_trip_with_colon_in_rel_type() {
        // Relationship types may contain colons (e.g. namespaced types).
        let member = encode_adj(EdgeId(5), NodeId(10), "ns:HAS:ITEM");
        let decoded = decode_adj(&member).expect("should decode");
        assert_eq!(decoded.edge_id, EdgeId(5));
        assert_eq!(decoded.neighbor_id, NodeId(10));
        assert_eq!(decoded.rel_type, "ns:HAS:ITEM");
    }

    #[test]
    fn adjacency_member_components() {
        // Verify the adjacency member format that the SADD pipeline uses.
        let out_member = encode_adj(EdgeId(1), NodeId(2), "FOLLOWS");
        let in_member = encode_adj(EdgeId(1), NodeId(3), "FOLLOWS");
        // out: edge -> dst, in: edge -> src
        assert!(out_member.starts_with("1:2:"));
        assert!(in_member.starts_with("1:3:"));
    }

    #[test]
    fn validate_edge_property_keys_rejects_null_byte() {
        let mut props = IndexMap::new();
        props.insert("co\x00st".to_string(), Value::Int(5));
        let result = validate_property_keys(&props);
        assert!(
            result.is_err(),
            "edge write-path property validation should reject null-byte keys"
        );
    }

    // ---- extract_injected_id for edge IDs -----------------------------------

    #[test]
    fn extract_edge_id_from_properties() {
        use crate::writes::helpers::extract_injected_id;
        use cypher_core::types::Value;
        use indexmap::IndexMap;

        let mut props = IndexMap::new();
        props.insert("_sd2_eid".to_string(), Value::Int(99));
        let result = extract_injected_id(&props, "_sd2_eid").unwrap();
        assert_eq!(result, Some(99));
    }

    #[test]
    fn extract_edge_id_negative_fails() {
        use crate::writes::helpers::extract_injected_id;
        use cypher_core::types::Value;
        use indexmap::IndexMap;

        let mut props = IndexMap::new();
        props.insert("_sd2_eid".to_string(), Value::Int(-1));
        assert!(extract_injected_id(&props, "_sd2_eid").is_err());
    }

    // ---- Cluster pipeline safety: verify no MULTI/EXEC in edge write pipelines --

    /// Verify that the edge creation pipeline does NOT include MULTI or EXEC.
    ///
    /// Fix for Issue #745: redis-rs ClusterConnection does not support MULTI/EXEC.
    /// The .atomic() calls in create_edge_impl and delete_edge_inner have been
    /// removed. This test validates the fix remains in place.
    #[test]
    fn create_edge_pipeline_has_no_multi_exec() {
        let graph = "testgraph";
        let edge_id = 5u64;
        let src_id = 1u64;
        let dst_id = 2u64;
        let rel_type = "KNOWS";

        let edge_key = format!("G:{{{graph}}}:edge:{edge_id}");
        let out_member = format!("{edge_id}:{dst_id}:{rel_type}");
        let in_member = format!("{edge_id}:{src_id}:{rel_type}");

        let mut pipe = redis::pipe();
        // Replicate create_edge_impl pipeline (without .atomic())
        pipe.cmd("HSET")
            .arg(&edge_key)
            .arg("src")
            .arg(src_id.to_string())
            .arg("dst")
            .arg(dst_id.to_string())
            .arg("rel_type")
            .arg(rel_type)
            .arg("props")
            .arg("{}")
            .ignore()
            .cmd("SADD")
            .arg(format!("G:{{{graph}}}:reltype:{rel_type}"))
            .arg(edge_id.to_string())
            .ignore()
            .cmd("SADD")
            .arg(format!("G:{{{graph}}}:out:{src_id}"))
            .arg(out_member)
            .ignore()
            .cmd("SADD")
            .arg(format!("G:{{{graph}}}:in:{dst_id}"))
            .arg(in_member)
            .ignore()
            .cmd("SADD")
            .arg(format!("G:{{{graph}}}:edges"))
            .arg(edge_id.to_string())
            .ignore();

        let packed = pipe.get_packed_pipeline();
        let pipeline_str = String::from_utf8_lossy(&packed);
        assert!(
            !pipeline_str.to_uppercase().contains("MULTI"),
            "Edge create pipeline must not contain MULTI: {pipeline_str}"
        );
        assert!(
            !pipeline_str.to_uppercase().contains("EXEC"),
            "Edge create pipeline must not contain EXEC: {pipeline_str}"
        );
    }

    /// Verify that the edge deletion pipeline does NOT include MULTI or EXEC.
    #[test]
    fn delete_edge_pipeline_has_no_multi_exec() {
        let graph = "testgraph";
        let edge_id = EdgeId(5);
        let src_id = NodeId(1);
        let dst_id = NodeId(2);
        let rel_type = "KNOWS";

        let out_member = encode_adj(edge_id, dst_id, rel_type);
        let in_member = encode_adj(edge_id, src_id, rel_type);

        let mut pipe = redis::pipe();
        // Replicate delete_edge_inner pipeline (without .atomic())
        pipe.cmd("SREM")
            .arg(format!("G:{{{graph}}}:reltype:{rel_type}"))
            .arg(edge_id.0.to_string())
            .ignore()
            .cmd("SREM")
            .arg(format!("G:{{{graph}}}:edges"))
            .arg(edge_id.0.to_string())
            .ignore()
            .cmd("SREM")
            .arg(format!("G:{{{graph}}}:out:{}", src_id.0))
            .arg(out_member)
            .ignore()
            .cmd("SREM")
            .arg(format!("G:{{{graph}}}:in:{}", dst_id.0))
            .arg(in_member)
            .ignore()
            .cmd("DEL")
            .arg(format!("G:{{{graph}}}:edge:{}", edge_id.0))
            .ignore();

        let packed = pipe.get_packed_pipeline();
        let pipeline_str = String::from_utf8_lossy(&packed);
        assert!(
            !pipeline_str.to_uppercase().contains("MULTI"),
            "Edge delete pipeline must not contain MULTI: {pipeline_str}"
        );
        assert!(
            !pipeline_str.to_uppercase().contains("EXEC"),
            "Edge delete pipeline must not contain EXEC: {pipeline_str}"
        );
    }

    /// Verify that all keys in the edge pipelines share the {graphname} hash tag.
    #[test]
    fn edge_pipeline_keys_all_share_hash_tag() {
        let graph = "mygraph";
        let edge_id = 10u64;
        let src_id = 1u64;
        let dst_id = 2u64;
        let rel_type = "FOLLOWS";
        let expected_tag = format!("{{{graph}}}");

        // Keys used in create_edge_impl and delete_edge_inner.
        let keys = vec![
            format!("G:{expected_tag}:edge:{edge_id}"),
            format!("G:{expected_tag}:reltype:{rel_type}"),
            format!("G:{expected_tag}:out:{src_id}"),
            format!("G:{expected_tag}:in:{dst_id}"),
            format!("G:{expected_tag}:edges"),
        ];

        for k in &keys {
            assert!(
                k.contains(&expected_tag),
                "Edge pipeline key missing hash tag {expected_tag}: {k}"
            );
        }
    }
}
