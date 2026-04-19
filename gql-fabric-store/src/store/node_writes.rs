// Node write operations: create, property updates, label changes, deletion.
//
// Each `*_impl` function is invoked from the `GraphStore` trait impl in
// `mod.rs`. Cypher strings come from the pure builders in `nodes.rs`.

use indexmap::IndexMap;
use std::collections::HashMap;

use serde_json::{json, Value as JValue};

use cypher_core::error::StorageError;
use cypher_core::storage::NodeId;
use cypher_core::types::Value;

use crate::parsing::user_props_to_json;
use crate::sanitize::sanitize_label;
use crate::{FabricGraphStore, PROP_GRAPH, PROP_LABELS, PROP_NODE_ID};

use super::helpers::{build_label_str, build_set_label_str};
use super::nodes;

pub(crate) fn create_node_impl(
    store: &FabricGraphStore,
    graph: &str,
    labels: Vec<String>,
    properties: IndexMap<String, Value>,
) -> Result<NodeId, StorageError> {
    let id = if let Some(Value::Int(i)) = properties.get(PROP_NODE_ID) {
        if *i < 0 {
            return Err(StorageError::Backend {
                msg: format!("_sd2_id must be non-negative, got {i}"),
            });
        }
        NodeId(*i as u64)
    } else {
        store.next_node_id(graph)?
    };

    let label_str = build_label_str(&labels);
    // See comment on the other call site — Vec<String> serialization
    // cannot fail; previously this silently wrote "[]" on an impossible
    // error, which would corrupt label data if it ever did fire.
    let labels_json =
        serde_json::to_string(&labels).expect("Vec<String> serialization is infallible");

    let mut all_props: HashMap<String, JValue> = user_props_to_json(&properties);
    all_props.insert(PROP_GRAPH.into(), json!(graph));
    all_props.insert(PROP_NODE_ID.into(), json!(id.0));
    all_props.insert(PROP_LABELS.into(), json!(labels_json));

    store.statement(
        &nodes::create_node_query(&label_str),
        json!({ "props": all_props }),
    )?;

    Ok(id)
}

pub(crate) fn set_node_properties_impl(
    store: &FabricGraphStore,
    graph: &str,
    id: NodeId,
    properties: IndexMap<String, Value>,
) -> Result<(), StorageError> {
    let props = user_props_to_json(&properties);
    store.statement(
        &nodes::set_node_properties_query(),
        json!({ "graph": graph, "id": id.0, "props": props }),
    )?;
    Ok(())
}

pub(crate) fn replace_node_properties_impl(
    store: &FabricGraphStore,
    graph: &str,
    id: NodeId,
    properties: IndexMap<String, Value>,
) -> Result<(), StorageError> {
    let mut props = user_props_to_json(&properties);
    props.insert(PROP_GRAPH.into(), json!(graph));
    props.insert(PROP_NODE_ID.into(), json!(id.0));
    store.statement(
        &nodes::replace_node_properties_query(),
        json!({ "graph": graph, "id": id.0, "props": props }),
    )?;
    store.refresh_labels_prop(graph, id)?;
    Ok(())
}

pub(crate) fn remove_node_property_impl(
    store: &FabricGraphStore,
    graph: &str,
    id: NodeId,
    key: &str,
) -> Result<(), StorageError> {
    let key = sanitize_label(key);
    store.statement(
        &nodes::remove_node_property_query(&key),
        json!({ "graph": graph, "id": id.0 }),
    )?;
    Ok(())
}

pub(crate) fn add_node_labels_impl(
    store: &FabricGraphStore,
    graph: &str,
    id: NodeId,
    labels: &[String],
) -> Result<(), StorageError> {
    let label_str = build_set_label_str(labels);
    if label_str.is_empty() {
        return Ok(());
    }
    store.statement(
        &nodes::add_node_labels_query(&label_str),
        json!({ "graph": graph, "id": id.0 }),
    )?;
    store.refresh_labels_prop(graph, id)?;
    Ok(())
}

pub(crate) fn remove_node_labels_impl(
    store: &FabricGraphStore,
    graph: &str,
    id: NodeId,
    labels: &[String],
) -> Result<(), StorageError> {
    let label_str = build_set_label_str(labels);
    if label_str.is_empty() {
        return Ok(());
    }
    store.statement(
        &nodes::remove_node_labels_query(&label_str),
        json!({ "graph": graph, "id": id.0 }),
    )?;
    store.refresh_labels_prop(graph, id)?;
    Ok(())
}

pub(crate) fn delete_node_impl(
    store: &FabricGraphStore,
    graph: &str,
    id: NodeId,
    detach: bool,
) -> Result<(), StorageError> {
    if !detach {
        let rows = store.query(
            &nodes::check_node_relationships_query(),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        let cnt = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if cnt > 0 {
            return Err(StorageError::Backend {
                msg: format!("node #{} has relationships; use DETACH DELETE", id.0),
            });
        }
    }
    store.statement(
        &nodes::delete_node_query(),
        json!({ "graph": graph, "id": id.0 }),
    )?;
    Ok(())
}
