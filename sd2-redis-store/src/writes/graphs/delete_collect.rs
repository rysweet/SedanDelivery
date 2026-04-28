// writes/graphs/delete_collect.rs -- Manifest scan helpers used by `delete_graph_impl`.
//
// These helpers walk the per-graph node and edge manifests in bounded chunks,
// renewing the write lock between chunks, and accumulate the auxiliary data
// (label set, reltype set, propidx-key set) the orchestrator in `delete.rs`
// needs to enumerate every per-graph key for a single cluster-safe DEL.

use std::collections::HashSet;

use cypher_core::error::StorageError;
use cypher_core::serde_value::json_to_props;

use crate::keys::{key_edge, key_node, key_propidx, value_to_index_key};
use crate::locked_conn::LockedRedisConn;
use crate::RedisGraphStore;

impl RedisGraphStore {
    /// Walk the node manifest in chunks, accumulating label names and
    /// property-index keys derived from the live property values.
    pub(super) fn delete_graph_collect_node_metadata(
        &self,
        conn: &mut LockedRedisConn,
        name: &str,
        node_ids: &[String],
        parsed_indexes: &[(String, String)],
        label_names: &mut HashSet<String>,
        propidx_keys: &mut HashSet<String>,
    ) -> Result<(), StorageError> {
        for chunk in node_ids.chunks(500) {
            conn.renew_lock()?;
            let mut valid_ids = Vec::new();
            let mut pipe = conn.pipeline();
            for id_str in chunk {
                match id_str.parse::<u64>() {
                    Ok(id) => {
                        valid_ids.push(id_str.as_str());
                        pipe.cmd("HMGET")
                            .key(self.kp(key_node(name, id)))
                            .arg("labels")
                            .arg("props");
                    }
                    Err(error) => {
                        tracing::warn!(
                            graph = %name,
                            node_id = %id_str,
                            %error,
                            "delete_graph skipping malformed node ID from nodes manifest"
                        );
                    }
                }
            }
            if valid_ids.is_empty() {
                continue;
            }
            let results: Vec<Vec<Option<String>>> =
                pipe.query(conn).map_err(crate::backend_storage_error)?;
            self.delete_graph_apply_node_chunk(
                name,
                &results,
                parsed_indexes,
                label_names,
                propidx_keys,
            )?;
        }
        Ok(())
    }

    /// Pure decoder: given the raw HMGET(labels, props) responses produced by
    /// one chunk of `delete_graph_collect_node_metadata`, fold their labels and
    /// matching property-index keys into the running accumulators.
    pub(super) fn delete_graph_apply_node_chunk(
        &self,
        name: &str,
        results: &[Vec<Option<String>>],
        parsed_indexes: &[(String, String)],
        label_names: &mut HashSet<String>,
        propidx_keys: &mut HashSet<String>,
    ) -> Result<(), StorageError> {
        for fields in results {
            let Some(labels_json) = fields.first().and_then(|value| value.as_ref()) else {
                continue;
            };
            let Some(props_json) = fields.get(1).and_then(|value| value.as_ref()) else {
                continue;
            };
            let labels: Vec<String> =
                serde_json::from_str(labels_json).map_err(|e| StorageError::Serialization {
                    msg: format!("delete_graph labels decode: {e}"),
                })?;
            for label in &labels {
                label_names.insert(label.clone());
            }
            let props = json_to_props(props_json).map_err(|e| StorageError::Serialization {
                msg: format!("delete_graph props decode: {e}"),
            })?;
            for (label, property) in parsed_indexes {
                if labels.iter().any(|node_label| node_label == label) {
                    if let Some(value) = props.get(property) {
                        if !value.is_null() {
                            propidx_keys.insert(self.kp(key_propidx(
                                name,
                                label,
                                property,
                                &value_to_index_key(value),
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Walk the edge manifest in chunks, accumulating relationship-type names
    /// observed on the persisted edge hashes.
    pub(super) fn delete_graph_collect_edge_reltypes(
        &self,
        conn: &mut LockedRedisConn,
        name: &str,
        edge_ids: &[String],
        reltype_names: &mut HashSet<String>,
    ) -> Result<(), StorageError> {
        for chunk in edge_ids.chunks(500) {
            conn.renew_lock()?;
            let mut pipe = conn.pipeline();
            let mut has_cmds = false;
            for id_str in chunk {
                match id_str.parse::<u64>() {
                    Ok(id) => {
                        pipe.cmd("HGET")
                            .key(self.kp(key_edge(name, id)))
                            .arg("rel_type");
                        has_cmds = true;
                    }
                    Err(error) => {
                        tracing::warn!(
                            graph = %name,
                            edge_id = %id_str,
                            %error,
                            "delete_graph skipping malformed edge ID from edges manifest"
                        );
                    }
                }
            }
            if !has_cmds {
                continue;
            }
            let results: Vec<Option<String>> =
                pipe.query(conn).map_err(crate::backend_storage_error)?;
            self.delete_graph_apply_edge_chunk(&results, reltype_names);
        }
        Ok(())
    }

    /// Pure decoder: given the raw HGET("rel_type") responses for one chunk,
    /// fold non-null reltype names into the running accumulator.
    pub(super) fn delete_graph_apply_edge_chunk(
        &self,
        results: &[Option<String>],
        reltype_names: &mut HashSet<String>,
    ) {
        for reltype in results.iter().flatten() {
            reltype_names.insert(reltype.clone());
        }
    }
}

#[cfg(test)]
mod apply_chunk_tests {
    //! Pure-data tests for `delete_graph_apply_node_chunk`. The pipelined
    //! `delete_graph_collect_*` paths require a `LockedRedisConn` and are
    //! exercised by the live-Redis suite in `tests/graph_lifecycle.rs`.

    use super::*;
    use crate::RedisGraphStore;

    fn store() -> RedisGraphStore {
        RedisGraphStore::new_for_testing()
    }

    #[test]
    fn apply_node_chunk_skips_rows_missing_labels_or_props() {
        let s = store();
        let mut labels = HashSet::new();
        let mut propidx = HashSet::new();
        let results = vec![
            vec![None, Some("{}".into())],      // missing labels -> skip
            vec![Some("[\"L\"]".into()), None], // missing props -> skip
            vec![],                             // empty row -> skip
        ];
        s.delete_graph_apply_node_chunk("g", &results, &[], &mut labels, &mut propidx)
            .unwrap();
        assert!(labels.is_empty());
        assert!(propidx.is_empty());
    }

    #[test]
    fn apply_node_chunk_accumulates_labels_and_propidx_keys() {
        let s = store();
        let mut labels = HashSet::new();
        let mut propidx = HashSet::new();
        let parsed_indexes = vec![("Person".to_string(), "name".to_string())];
        let results = vec![vec![
            Some("[\"Person\",\"User\"]".into()),
            Some("{\"name\":\"Alice\",\"age\":30}".into()),
        ]];
        s.delete_graph_apply_node_chunk("g", &results, &parsed_indexes, &mut labels, &mut propidx)
            .unwrap();
        assert!(labels.contains("Person"));
        assert!(labels.contains("User"));
        assert_eq!(propidx.len(), 1, "propidx={propidx:?}");
    }

    #[test]
    fn apply_node_chunk_skips_propidx_when_label_absent_on_node() {
        let s = store();
        let mut labels = HashSet::new();
        let mut propidx = HashSet::new();
        let parsed_indexes = vec![("Person".to_string(), "name".to_string())];
        let results = vec![vec![
            Some("[\"Movie\"]".into()),
            Some("{\"name\":\"Matrix\"}".into()),
        ]];
        s.delete_graph_apply_node_chunk("g", &results, &parsed_indexes, &mut labels, &mut propidx)
            .unwrap();
        assert!(labels.contains("Movie"));
        assert!(propidx.is_empty(), "Person index must not match Movie node");
    }

    #[test]
    fn apply_node_chunk_skips_null_property_values() {
        let s = store();
        let mut labels = HashSet::new();
        let mut propidx = HashSet::new();
        let parsed_indexes = vec![("Person".to_string(), "name".to_string())];
        let results = vec![vec![
            Some("[\"Person\"]".into()),
            Some("{\"name\":null}".into()),
        ]];
        s.delete_graph_apply_node_chunk("g", &results, &parsed_indexes, &mut labels, &mut propidx)
            .unwrap();
        assert!(
            propidx.is_empty(),
            "null property must not register propidx"
        );
    }

    #[test]
    fn apply_node_chunk_returns_serialization_error_on_bad_labels_json() {
        let s = store();
        let mut labels = HashSet::new();
        let mut propidx = HashSet::new();
        let results = vec![vec![Some("not-json".into()), Some("{}".into())]];
        let err = s
            .delete_graph_apply_node_chunk("g", &results, &[], &mut labels, &mut propidx)
            .unwrap_err();
        assert!(matches!(err, StorageError::Serialization { .. }));
    }

    #[test]
    fn apply_node_chunk_returns_serialization_error_on_bad_props_json() {
        let s = store();
        let mut labels = HashSet::new();
        let mut propidx = HashSet::new();
        let results = vec![vec![Some("[\"L\"]".into()), Some("not-json".into())]];
        let err = s
            .delete_graph_apply_node_chunk("g", &results, &[], &mut labels, &mut propidx)
            .unwrap_err();
        assert!(matches!(err, StorageError::Serialization { .. }));
    }

    #[test]
    fn apply_node_chunk_skips_property_not_in_props_map() {
        let s = store();
        let mut labels = HashSet::new();
        let mut propidx = HashSet::new();
        let parsed_indexes = vec![("Person".to_string(), "missing_prop".to_string())];
        let results = vec![vec![
            Some("[\"Person\"]".into()),
            Some("{\"name\":\"Alice\"}".into()),
        ]];
        s.delete_graph_apply_node_chunk("g", &results, &parsed_indexes, &mut labels, &mut propidx)
            .unwrap();
        assert!(propidx.is_empty());
    }

    #[test]
    fn apply_edge_chunk_inserts_non_null_reltypes() {
        let s = store();
        let mut reltypes = HashSet::new();
        let results = vec![
            Some("KNOWS".to_string()),
            None, // skipped
            Some("FOLLOWS".to_string()),
            Some("KNOWS".to_string()), // dedup via HashSet
        ];
        s.delete_graph_apply_edge_chunk(&results, &mut reltypes);
        assert_eq!(reltypes.len(), 2);
        assert!(reltypes.contains("KNOWS"));
        assert!(reltypes.contains("FOLLOWS"));
    }

    #[test]
    fn apply_edge_chunk_handles_all_none() {
        let s = store();
        let mut reltypes = HashSet::new();
        s.delete_graph_apply_edge_chunk(&[None, None], &mut reltypes);
        assert!(reltypes.is_empty());
    }

    #[test]
    fn apply_edge_chunk_no_op_on_empty() {
        let s = store();
        let mut reltypes = HashSet::new();
        s.delete_graph_apply_edge_chunk(&[], &mut reltypes);
        assert!(reltypes.is_empty());
    }
}
