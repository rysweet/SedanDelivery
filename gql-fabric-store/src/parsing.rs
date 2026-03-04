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
        let rel_type = v["type"].as_str().unwrap_or("UNKNOWN").to_string();
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
    obj.iter()
        .filter(|(k, _)| !k.starts_with("_sd2") && !k.starts_with("__SD2"))
        .filter_map(|(k, v)| serde_value::json_to_value(v).map(|val| (k.clone(), val)))
        .collect()
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
                serde_value::value_to_json(v).unwrap_or(JValue::Null),
            )
        })
        .collect()
}

/// Convert adjacency query rows (eid, nbr, rt) to AdjEntry vec.
pub(crate) fn rows_to_adj(rows: Vec<Vec<JValue>>) -> Result<Vec<AdjEntry>, StorageError> {
    rows.into_iter()
        .filter_map(|r| {
            let eid = r.first()?.as_u64().map(EdgeId)?;
            let nbr = r.get(1)?.as_u64().map(NodeId)?;
            let rt = r.get(2)?.as_str()?.to_string();
            Some(Ok(AdjEntry {
                edge_id: eid,
                neighbor_id: nbr,
                rel_type: rt,
            }))
        })
        .collect()
}
