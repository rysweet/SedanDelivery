// writes/graphs/ -- Graph lifecycle write operations for RedisGraphStore.
//
// Split by responsibility:
//   - create:         create_graph (registry insert + name validation)
//   - delete:         delete_graph orchestration (preflight, marker, DEL, dereg)
//   - delete_collect: chunked manifest scans used by delete to derive label,
//                     reltype, and propidx-key sets
//
// `mod.rs` keeps the module wiring and the source-level/contract tests that
// inspect the cluster-safe ordering and typed-error guarantees of the
// orchestrator.

mod create;
mod delete;
mod delete_collect;

#[cfg(test)]
mod tests {
    use cypher_core::storage::validate_graph_name;

    /// Concatenated source for the orchestrator and its manifest-scan helpers.
    /// Tests below inspect this combined source so the assertions remain valid
    /// after the helpers were extracted out of the original `graphs.rs`.
    const DELETE_SOURCE: &str = concat!(
        include_str!("delete.rs"),
        "\n",
        include_str!("delete_collect.rs"),
    );

    // ---- Graph name validation (pure logic, no Redis needed) ----------------

    #[test]
    fn valid_graph_names() {
        assert!(validate_graph_name("mydb").is_ok());
        assert!(validate_graph_name("test_graph").is_ok());
        assert!(validate_graph_name("g1").is_ok());
        assert!(validate_graph_name("a").is_ok());
    }

    #[test]
    fn empty_graph_name_rejected() {
        assert!(validate_graph_name("").is_err());
    }

    // ---- delete_graph key construction (set-enumeration path) ---------------

    #[test]
    fn delete_graph_fixed_keys_carry_hash_tag() {
        // The fixed keys built by delete_graph_impl must all include the
        // {name} hash tag so they route to the same Redis Cluster slot.
        use crate::keys::{
            key_edge_seq, key_edges_set, key_graph_deleting, key_indexes, key_label_names,
            key_node_seq, key_nodes_set, key_propidx_manifest, key_reltype_names,
        };
        let name = "testdb";
        let tag = format!("{{{name}}}");
        for key in [
            key_node_seq(name),
            key_edge_seq(name),
            key_nodes_set(name),
            key_edges_set(name),
            key_indexes(name),
            key_graph_deleting(name),
            key_label_names(name),
            key_reltype_names(name),
            key_propidx_manifest(name),
        ] {
            assert!(
                key.contains(&tag),
                "key '{key}' must contain hash tag '{tag}'"
            );
        }
    }

    #[test]
    fn delete_graph_derived_keys_carry_hash_tag() {
        // Per-node, per-edge, registry-backed, and propidx keys derived during
        // delete_graph_impl must all include the graph hash tag.
        use crate::keys::{
            key_edge, key_in, key_label, key_node, key_out, key_propidx, key_reltype,
        };
        let name = "testdb";
        let tag = format!("{{{name}}}");
        for key in [
            key_node(name, 1),
            key_out(name, 1),
            key_in(name, 1),
            key_edge(name, 1),
            key_label(name, "Person"),
            key_reltype(name, "KNOWS"),
            key_propidx(name, "Person", "name", "s:Alice"),
        ] {
            assert!(
                key.contains(&tag),
                "key '{key}' must contain hash tag '{tag}'"
            );
        }
    }

    #[test]
    fn max_nodes_for_delete_constant() {
        // Verify the guard is a positive, bounded value.
        const { assert!(crate::MAX_NODES_FOR_DELETE > 0) };
        const { assert!(crate::MAX_NODES_FOR_DELETE <= 1_000_000) };
    }

    #[test]
    fn delete_graph_checks_size_before_registry_removal() {
        let delete_impl = DELETE_SOURCE
            .split("pub(crate) fn delete_graph_impl")
            .nth(1)
            .expect("delete_graph_impl should exist");
        let node_count_guard = delete_impl
            .find("let node_count: usize = redis::cmd(\"SCARD\")")
            .expect("node-count preflight should exist");
        let edge_count_guard = delete_impl
            .find("let edge_count: usize = redis::cmd(\"SCARD\")")
            .expect("edge-count preflight should exist");
        let first_manifest_fetch = delete_impl
            .find("let node_ids: Vec<String> = redis::cmd(\"SMEMBERS\")")
            .expect("manifest fetch should exist");
        let deregister = delete_impl
            .rfind("redis::cmd(\"SREM\")")
            .expect("graph deregistration should exist");
        assert!(
            node_count_guard < first_manifest_fetch,
            "delete_graph_impl must preflight node cardinality before loading manifests"
        );
        assert!(
            edge_count_guard < first_manifest_fetch,
            "delete_graph_impl must preflight edge cardinality before loading manifests"
        );
        assert!(
            node_count_guard < edge_count_guard,
            "node cardinality check should happen before edge cardinality check"
        );
        assert!(
            node_count_guard < deregister,
            "delete_graph_impl must reject oversized graphs before removing them from the registry"
        );
        assert!(
            edge_count_guard < deregister,
            "delete_graph_impl must reject oversized graphs before removing them from the registry"
        );
    }

    #[test]
    fn delete_graph_checks_edge_manifest_size() {
        let delete_impl = DELETE_SOURCE
            .split("pub(crate) fn delete_graph_impl")
            .nth(1)
            .expect("delete_graph_impl should exist");
        let edge_count_guard = delete_impl
            .find("let edge_count: usize = redis::cmd(\"SCARD\")")
            .expect("edge-count preflight should exist");
        let node_manifest_fetch = delete_impl
            .find("let node_ids: Vec<String> = redis::cmd(\"SMEMBERS\")")
            .expect("node manifest fetch should exist");
        let edge_manifest_fetch = delete_impl
            .find("let edge_ids: Vec<String> = redis::cmd(\"SMEMBERS\")")
            .expect("edge manifest fetch should exist");
        assert!(
            delete_impl.contains("if edge_count > crate::MAX_NODES_FOR_DELETE"),
            "delete_graph_impl must bound the edge manifest before building cleanup state"
        );
        assert!(
            edge_count_guard < node_manifest_fetch,
            "delete_graph_impl must preflight edge cardinality before loading the node manifest"
        );
        assert!(
            edge_count_guard < edge_manifest_fetch,
            "delete_graph_impl must preflight edge cardinality before loading the edge manifest"
        );
    }

    #[test]
    fn delete_graph_logs_manifest_parse_failures_instead_of_silently_skipping() {
        let delete_impl = DELETE_SOURCE
            .split("pub(crate) fn delete_graph_impl")
            .nth(1)
            .expect("delete_graph_impl should exist");

        for expected in [
            "delete_graph skipping malformed node ID from nodes manifest",
            "delete_graph skipping malformed edge ID from edges manifest",
            "delete_graph omitting malformed node ID during key construction",
            "delete_graph omitting malformed edge ID during key construction",
        ] {
            assert!(
                delete_impl.contains(expected),
                "delete_graph_impl must log parse failures instead of silently skipping: {expected}"
            );
        }
    }
}

// ── WS-F (#755): Typed error for oversized delete — contract tests ────────────
//
// These tests verify that delete_graph_impl emits StorageError::DeleteGraphTooLarge
// (not the generic StorageError::Backend) when node/edge counts exceed the limit.

#[cfg(test)]
mod ws_f_typed_error_source_contract_tests {
    const DELETE_SOURCE: &str = include_str!("delete.rs");

    /// Verify delete_graph_impl source emits DeleteGraphTooLarge for node overflow.
    #[test]
    fn delete_graph_node_overflow_uses_typed_error_not_backend() {
        let delete_impl = DELETE_SOURCE
            .split("pub(crate) fn delete_graph_impl")
            .nth(1)
            .expect("delete_graph_impl must exist");

        // After WS-F: the node-overflow branch must use DeleteGraphTooLarge,
        // NOT the generic Backend error.
        assert!(
            delete_impl.contains("DeleteGraphTooLarge"),
            "delete_graph_impl must emit DeleteGraphTooLarge (not Backend) for node overflow; \
             got: {}",
            &delete_impl[..delete_impl.find('\n').unwrap_or(200).min(200)]
        );
    }

    /// Verify delete_graph_impl source emits DeleteGraphTooLarge for edge overflow.
    #[test]
    fn delete_graph_edge_overflow_uses_typed_error_not_backend() {
        let delete_impl = DELETE_SOURCE
            .split("pub(crate) fn delete_graph_impl")
            .nth(1)
            .expect("delete_graph_impl must exist");

        // Count occurrences of DeleteGraphTooLarge — should be at least 2
        // (one for nodes, one for edges).
        let count = delete_impl.matches("DeleteGraphTooLarge").count();
        assert!(
            count >= 2,
            "delete_graph_impl must have at least 2 DeleteGraphTooLarge emissions \
             (one for nodes, one for edges), found {count}"
        );
    }

    /// Verify that the oversized-graph branches no longer use the generic Backend error
    /// with the old hard-coded message format.
    #[test]
    fn delete_graph_overflow_does_not_use_legacy_backend_message() {
        let delete_impl = DELETE_SOURCE
            .split("pub(crate) fn delete_graph_impl")
            .nth(1)
            .expect("delete_graph_impl must exist");

        // The old message pattern must be removed after WS-F.
        assert!(
            !delete_impl.contains("exceeding MAX_NODES_FOR_DELETE"),
            "delete_graph_impl must not use the old Backend error message format after WS-F; \
             update both branches to use DeleteGraphTooLarge"
        );
    }

    /// Verify that the `resource` field discriminates between "nodes" and "edges".
    #[test]
    fn delete_graph_overflow_sets_correct_resource_field() {
        let delete_impl = DELETE_SOURCE
            .split("pub(crate) fn delete_graph_impl")
            .nth(1)
            .expect("delete_graph_impl must exist");

        // After WS-F, both "nodes" and "edges" resource strings must appear.
        assert!(
            delete_impl.contains("\"nodes\""),
            "delete_graph_impl must set resource: \"nodes\" for node overflow"
        );
        assert!(
            delete_impl.contains("\"edges\""),
            "delete_graph_impl must set resource: \"edges\" for edge overflow"
        );
    }

    /// Verify that _deleting marker is set BEFORE the DEL of per-graph keys,
    /// and AFTER lock acquisition — the orphan-safety placement requirement.
    #[test]
    fn delete_graph_sets_deleting_marker_before_per_graph_del() {
        let delete_impl = DELETE_SOURCE
            .split("pub(crate) fn delete_graph_impl")
            .nth(1)
            .expect("delete_graph_impl must exist");

        let deleting_set_pos = delete_impl
            .find("redis::cmd(\"SET\")")
            .expect("_deleting SET must be present in delete_graph_impl");

        let keys_del_pos = delete_impl
            .rfind("redis::cmd(\"DEL\")")
            .expect("DEL of per-graph keys must be present in delete_graph_impl");

        assert!(
            deleting_set_pos < keys_del_pos,
            "_deleting marker must be SET before the bulk per-graph DEL for crash-safety"
        );
    }
}
