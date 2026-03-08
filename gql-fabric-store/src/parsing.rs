// Row-to-domain parsing: convert Fabric JSON rows into NodeData / EdgeData / AdjEntry.

use std::collections::HashMap;

use serde_json::Value as JValue;

use gql_core::error::StorageError;
use gql_core::serde_value;
use gql_core::storage::{AdjEntry, EdgeData, EdgeId, NodeData, NodeId};
use gql_core::types::Value;

use crate::{FabricGraphStore, PROP_DST, PROP_EDGE_ID, PROP_LABELS, PROP_NODE_ID, PROP_SRC};

impl FabricGraphStore {
    pub(crate) fn row_to_node_data(
        &self,
        row: &[JValue],
    ) -> Result<Option<NodeData>, StorageError> {
        let v = match row.first() {
            Some(v) if !v.is_null() => v,
            _ => return Ok(None),
        };
        let id_raw =
            v["properties"][PROP_NODE_ID]
                .as_u64()
                .ok_or_else(|| StorageError::Serialization {
                    msg: "node missing _sd2_id".into(),
                })?;
        let labels: Vec<String> = v["properties"][PROP_LABELS]
            .as_str()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_else(|| {
                // Fall back to Fabric's native labels, filtering internal ones
                v["labels"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|l| l.as_str())
                            .filter(|l| !l.starts_with("__SD2"))
                            .map(|l| l.to_string())
                            .collect()
                    })
                    .unwrap_or_default()
            });
        let properties = extract_user_props(&v["properties"]);
        Ok(Some(NodeData {
            id: NodeId(id_raw),
            labels,
            properties,
        }))
    }

    pub(crate) fn row_to_edge_data(
        &self,
        row: &[JValue],
    ) -> Result<Option<EdgeData>, StorageError> {
        let v = match row.first() {
            Some(v) if !v.is_null() => v,
            _ => return Ok(None),
        };
        let id_raw =
            v["properties"][PROP_EDGE_ID]
                .as_u64()
                .ok_or_else(|| StorageError::Serialization {
                    msg: "edge missing _sd2_eid".into(),
                })?;
        let src =
            v["properties"][PROP_SRC]
                .as_u64()
                .ok_or_else(|| StorageError::Serialization {
                    msg: "edge missing _sd2_src".into(),
                })?;
        let dst =
            v["properties"][PROP_DST]
                .as_u64()
                .ok_or_else(|| StorageError::Serialization {
                    msg: "edge missing _sd2_dst".into(),
                })?;
        let rel_type = v["type"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| StorageError::Serialization {
                msg: format!("edge #{id_raw} missing relationship type (corrupted data)"),
            })?;
        let properties = extract_user_props(&v["properties"]);
        Ok(Some(EdgeData {
            id: EdgeId(id_raw),
            rel_type,
            src: NodeId(src),
            dst: NodeId(dst),
            properties,
        }))
    }
}

/// Extract user-visible properties, stripping all `_sd2_*` internal keys.
pub(crate) fn extract_user_props(props: &JValue) -> HashMap<String, Value> {
    let obj = match props.as_object() {
        Some(o) => o,
        None => return HashMap::new(),
    };
    let mut result = HashMap::new();
    for (k, v) in obj
        .iter()
        .filter(|(k, _)| !k.starts_with("_sd2") && !k.starts_with("__SD2"))
    {
        match serde_value::json_to_value(v) {
            Some(val) => {
                result.insert(k.clone(), val);
            }
            None => {
                tracing::warn!(key = %k, json = %v, "json_to_value conversion failed, skipping property");
            }
        }
    }
    result
}

/// Convert a user property map to JSON, skipping internal SD2 keys.
/// Non-finite floats are silently converted to JSON null.
pub(crate) fn user_props_to_json(props: &HashMap<String, Value>) -> HashMap<String, JValue> {
    props
        .iter()
        .filter(|(k, _)| !k.starts_with("_sd2") && !k.starts_with("__SD2"))
        .map(|(k, v)| {
            (
                k.clone(),
                serde_value::value_to_json(v).unwrap_or_else(|e| {
                    tracing::warn!(key = %k, error = %e, "value_to_json failed, using null");
                    JValue::Null
                }),
            )
        })
        .collect()
}

/// Convert adjacency query rows (eid, nbr, rt) to AdjEntry vec.
pub(crate) fn rows_to_adj(rows: Vec<Vec<JValue>>) -> Result<Vec<AdjEntry>, StorageError> {
    let mut entries = Vec::new();
    for r in rows {
        let parsed = (|| {
            let eid = r.first()?.as_u64().map(EdgeId)?;
            let nbr = r.get(1)?.as_u64().map(NodeId)?;
            let rt = r.get(2)?.as_str()?.to_string();
            Some(AdjEntry {
                edge_id: eid,
                neighbor_id: nbr,
                rel_type: rt,
            })
        })();
        match parsed {
            Some(entry) => entries.push(entry),
            None => {
                tracing::warn!(row = ?r, "dropping malformed adjacency row");
            }
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── extract_user_props ───────────────────────────────────────────────────

    #[test]
    fn extract_user_props_filters_sd2_keys() {
        let props = json!({
            "name": "Alice",
            "age": 30,
            "_sd2_id": 1,
            "_sd2_graph": "g",
            "__SD2Seq": true,
            "city": "NYC"
        });
        let result = extract_user_props(&props);
        assert_eq!(result.len(), 3);
        assert!(result.contains_key("name"));
        assert!(result.contains_key("age"));
        assert!(result.contains_key("city"));
        assert!(!result.contains_key("_sd2_id"));
        assert!(!result.contains_key("_sd2_graph"));
        assert!(!result.contains_key("__SD2Seq"));
    }

    #[test]
    fn extract_user_props_empty_object() {
        let props = json!({});
        let result = extract_user_props(&props);
        assert!(result.is_empty());
    }

    #[test]
    fn extract_user_props_non_object_returns_empty() {
        assert!(extract_user_props(&json!("not an object")).is_empty());
        assert!(extract_user_props(&json!(null)).is_empty());
        assert!(extract_user_props(&json!([1, 2, 3])).is_empty());
    }

    #[test]
    fn extract_user_props_preserves_types() {
        let props = json!({
            "name": "Bob",
            "age": 25,
            "active": true,
            "score": 99.5
        });
        let result = extract_user_props(&props);
        assert_eq!(result.len(), 4);
        assert_eq!(result["name"], Value::Str("Bob".into()));
        assert_eq!(result["age"], Value::Int(25));
        assert_eq!(result["active"], Value::Bool(true));
        assert_eq!(result["score"], Value::Float(99.5));
    }

    // ── user_props_to_json ──────────────────────────────────────────────────

    #[test]
    fn user_props_to_json_filters_internal_keys() {
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Carol".into()));
        props.insert("_sd2_id".to_string(), Value::Int(7));
        props.insert("__SD2graph".to_string(), Value::Str("g".into()));
        let result = user_props_to_json(&props);
        assert_eq!(result.len(), 1);
        assert_eq!(result["name"], json!("Carol"));
    }

    #[test]
    fn user_props_to_json_empty() {
        let props = HashMap::new();
        let result = user_props_to_json(&props);
        assert!(result.is_empty());
    }

    #[test]
    fn user_props_to_json_non_finite_float_becomes_null() {
        let mut props = HashMap::new();
        props.insert("x".to_string(), Value::Float(f64::NAN));
        let result = user_props_to_json(&props);
        assert!(result["x"].is_null());
    }

    // ── rows_to_adj ─────────────────────────────────────────────────────────

    #[test]
    fn rows_to_adj_valid() {
        let rows = vec![
            vec![json!(1), json!(10), json!("KNOWS")],
            vec![json!(2), json!(20), json!("LIKES")],
        ];
        let result = rows_to_adj(rows).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].edge_id, EdgeId(1));
        assert_eq!(result[0].neighbor_id, NodeId(10));
        assert_eq!(result[0].rel_type, "KNOWS");
        assert_eq!(result[1].edge_id, EdgeId(2));
        assert_eq!(result[1].neighbor_id, NodeId(20));
        assert_eq!(result[1].rel_type, "LIKES");
    }

    #[test]
    fn rows_to_adj_empty() {
        let result = rows_to_adj(vec![]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn rows_to_adj_skips_malformed_rows() {
        let rows = vec![
            vec![json!(1), json!(10), json!("KNOWS")],
            vec![json!("bad"), json!(20), json!("X")],
            vec![json!(3)],
            vec![],
            vec![json!(4), json!(40), json!(null)],
        ];
        let result = rows_to_adj(rows).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].edge_id, EdgeId(1));
    }
}
