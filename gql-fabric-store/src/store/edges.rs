// Edge operations: reads, writes, property updates, deletion.
//
// Each `*_impl` function is called by the GraphStore trait impl in mod.rs.
// The `*_query` functions are pure Cypher builders, testable without Fabric.

use std::collections::HashMap;

use serde_json::{json, Value as JValue};

use gql_core::error::StorageError;
use gql_core::storage::{EdgeData, EdgeId, GraphStore, NodeId};
use gql_core::types::Value;

use crate::parsing::user_props_to_json;
use crate::sanitize::sanitize_label;
use crate::{FabricGraphStore, PROP_DST, PROP_EDGE_ID, PROP_GRAPH, PROP_NODE_ID, PROP_SRC};

// ── Cypher query builders ───────────────────────────────────────────────────

/// Cypher: get a single edge by graph + edge ID.
pub(crate) fn get_edge_query() -> String {
    format!("MATCH ()-[r {{`{PROP_GRAPH}`: $graph, `{PROP_EDGE_ID}`: $id}}]->() RETURN r LIMIT 1")
}

/// Cypher: UNWIND batch of IDs and OPTIONAL MATCH edges.
pub(crate) fn get_edges_batch_query() -> String {
    format!(
        "UNWIND $ids AS target \
         OPTIONAL MATCH ()-[r {{`{PROP_GRAPH}`: $graph, `{PROP_EDGE_ID}`: target}}]->() \
         RETURN r ORDER BY target"
    )
}

/// Cypher: find edges by relationship type (sanitized), returning their IDs.
pub(crate) fn find_edges_by_type_query(rt: &str) -> String {
    format!(
        "MATCH ()-[r:`{rt}` {{`{PROP_GRAPH}`: $graph}}]->() RETURN r.`{PROP_EDGE_ID}` AS id"
    )
}

/// Cypher: all edge IDs in a graph.
pub(crate) fn all_edge_ids_query() -> String {
    format!("MATCH ()-[r {{`{PROP_GRAPH}`: $graph}}]->() RETURN r.`{PROP_EDGE_ID}` AS id")
}

/// Cypher: CREATE an edge between two matched nodes.
pub(crate) fn create_edge_query(rt: &str) -> String {
    format!(
        "MATCH (a {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $src}}), \
               (b {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $dst}}) \
         CREATE (a)-[r:`{rt}` $props]->(b) \
         RETURN r.`{PROP_EDGE_ID}` AS id"
    )
}

/// Cypher: SET r += $props (merge edge properties).
pub(crate) fn set_edge_properties_query() -> String {
    format!("MATCH ()-[r {{`{PROP_GRAPH}`: $graph, `{PROP_EDGE_ID}`: $id}}]->() SET r += $props")
}

/// Cypher: SET r = $props (replace all edge properties).
pub(crate) fn replace_edge_properties_query() -> String {
    format!(
        "MATCH ()-[r {{`{PROP_GRAPH}`: $graph, `{PROP_EDGE_ID}`: $id}}]->() SET r = $props"
    )
}

/// Cypher: REMOVE a single property from an edge.
pub(crate) fn remove_edge_property_query(key: &str) -> String {
    format!("MATCH ()-[r {{`{PROP_GRAPH}`: $graph, `{PROP_EDGE_ID}`: $id}}]->() REMOVE r.`{key}`")
}

/// Cypher: DELETE an edge.
pub(crate) fn delete_edge_query() -> String {
    format!("MATCH ()-[r {{`{PROP_GRAPH}`: $graph, `{PROP_EDGE_ID}`: $id}}]->() DELETE r")
}

// ── Implementation functions (called by GraphStore trait impl) ──────────────

pub(super) fn get_edge_impl(
    store: &FabricGraphStore,
    graph: &str,
    id: EdgeId,
) -> Result<Option<EdgeData>, StorageError> {
    let rows = store.query(
        &get_edge_query(),
        json!({ "graph": graph, "id": id.0 }),
    )?;
    rows.first()
        .map(|r| store.row_to_edge_data(r))
        .unwrap_or(Ok(None))
}

pub(super) fn get_edges_batch_impl(
    store: &FabricGraphStore,
    graph: &str,
    ids: &[EdgeId],
) -> Result<Vec<Option<EdgeData>>, StorageError> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let id_list: JValue = ids.iter().map(|e| e.0).collect::<Vec<_>>().into();
    let rows = store.query(
        &get_edges_batch_query(),
        json!({ "graph": graph, "ids": id_list }),
    )?;
    rows.iter().map(|r| store.row_to_edge_data(r)).collect()
}

pub(super) fn find_edges_by_type_impl(
    store: &FabricGraphStore,
    graph: &str,
    rel_type: &str,
) -> Result<Vec<EdgeId>, StorageError> {
    let rt = sanitize_label(rel_type);
    if rt.is_empty() {
        return Err(StorageError::Backend {
            msg: format!(
                "invalid relationship type: sanitize_label produced empty string from {:?}",
                rel_type
            ),
        });
    }
    let rows = store.query(
        &find_edges_by_type_query(&rt),
        json!({ "graph": graph }),
    )?;
    Ok(rows
        .iter()
        .filter_map(|r| r.first()?.as_u64().map(EdgeId))
        .collect::<Vec<_>>())
}

pub(super) fn all_edge_ids_impl(
    store: &FabricGraphStore,
    graph: &str,
) -> Result<Vec<EdgeId>, StorageError> {
    let rows = store.query(
        &all_edge_ids_query(),
        json!({ "graph": graph }),
    )?;
    Ok(rows
        .iter()
        .filter_map(|r| r.first()?.as_u64().map(EdgeId))
        .collect::<Vec<_>>())
}

pub(super) fn create_edge_impl(
    store: &FabricGraphStore,
    graph: &str,
    src: NodeId,
    dst: NodeId,
    rel_type: String,
    properties: HashMap<String, Value>,
) -> Result<EdgeId, StorageError> {
    let id = if let Some(Value::Int(i)) = properties.get(PROP_EDGE_ID) {
        if *i < 0 {
            return Err(StorageError::Backend {
                msg: format!("_sd2_eid must be non-negative, got {i}"),
            });
        }
        EdgeId(*i as u64)
    } else {
        store.next_edge_id(graph)?
    };

    let rt = sanitize_label(&rel_type);
    let mut all_props: HashMap<String, JValue> = user_props_to_json(&properties);
    all_props.insert(PROP_GRAPH.into(), json!(graph));
    all_props.insert(PROP_EDGE_ID.into(), json!(id.0));
    all_props.insert(PROP_SRC.into(), json!(src.0));
    all_props.insert(PROP_DST.into(), json!(dst.0));

    let rows = store.statement(
        &create_edge_query(&rt),
        json!({ "graph": graph, "src": src.0, "dst": dst.0, "props": all_props }),
    )?;

    if rows.is_empty() {
        return Err(StorageError::Backend {
            msg: format!(
                "create_edge: src {src:?} or dst {dst:?} not found in graph '{graph}'"
            ),
        });
    }
    Ok(id)
}

pub(super) fn set_edge_properties_impl(
    store: &FabricGraphStore,
    graph: &str,
    id: EdgeId,
    properties: HashMap<String, Value>,
) -> Result<(), StorageError> {
    let props = user_props_to_json(&properties);
    store.statement(
        &set_edge_properties_query(),
        json!({ "graph": graph, "id": id.0, "props": props }),
    )?;
    Ok(())
}

pub(super) fn replace_edge_properties_impl(
    store: &FabricGraphStore,
    graph: &str,
    id: EdgeId,
    properties: HashMap<String, Value>,
) -> Result<(), StorageError> {
    let edge = store
        .get_edge(graph, id)?
        .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
    let mut props = user_props_to_json(&properties);
    props.insert(PROP_GRAPH.into(), json!(graph));
    props.insert(PROP_EDGE_ID.into(), json!(id.0));
    props.insert(PROP_SRC.into(), json!(edge.src.0));
    props.insert(PROP_DST.into(), json!(edge.dst.0));
    store.statement(
        &replace_edge_properties_query(),
        json!({ "graph": graph, "id": id.0, "props": props }),
    )?;
    Ok(())
}

pub(super) fn remove_edge_property_impl(
    store: &FabricGraphStore,
    graph: &str,
    id: EdgeId,
    key: &str,
) -> Result<(), StorageError> {
    let key = sanitize_label(key);
    store.statement(
        &remove_edge_property_query(&key),
        json!({ "graph": graph, "id": id.0 }),
    )?;
    Ok(())
}

pub(super) fn delete_edge_impl(
    store: &FabricGraphStore,
    graph: &str,
    id: EdgeId,
) -> Result<(), StorageError> {
    store.statement(
        &delete_edge_query(),
        json!({ "graph": graph, "id": id.0 }),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Edge read queries ───────────────────────────────────────────────────

    #[test]
    fn get_edge_query_structure() {
        let q = get_edge_query();
        assert!(q.starts_with("MATCH ()-[r {"), "got: {q}");
        assert!(q.contains(PROP_GRAPH), "missing graph prop: {q}");
        assert!(q.contains(PROP_EDGE_ID), "missing edge id prop: {q}");
        assert!(q.contains("$graph"), "missing $graph param: {q}");
        assert!(q.contains("$id"), "missing $id param: {q}");
        assert!(q.contains("RETURN r LIMIT 1"), "missing RETURN: {q}");
        assert!(q.contains("->()"), "missing directed arrow: {q}");
    }

    #[test]
    fn get_edges_batch_query_structure() {
        let q = get_edges_batch_query();
        assert!(q.contains("UNWIND $ids AS target"), "missing UNWIND: {q}");
        assert!(q.contains("OPTIONAL MATCH"), "missing OPTIONAL MATCH: {q}");
        assert!(q.contains("ORDER BY target"), "missing ORDER BY: {q}");
        assert!(q.contains("->()"), "missing directed arrow: {q}");
    }

    #[test]
    fn find_edges_by_type_query_interpolates_type() {
        let q = find_edges_by_type_query("KNOWS");
        assert!(q.contains(":`KNOWS`"), "missing type in: {q}");
        assert!(q.contains("$graph"), "missing $graph: {q}");
        assert!(q.contains("AS id"), "missing id alias: {q}");
    }

    #[test]
    fn all_edge_ids_query_structure() {
        let q = all_edge_ids_query();
        assert!(q.contains("MATCH ()-[r {"), "got: {q}");
        assert!(q.contains("$graph"), "missing $graph: {q}");
        assert!(q.contains("AS id"), "missing id alias: {q}");
    }

    // ── Edge write queries ──────────────────────────────────────────────────

    #[test]
    fn create_edge_query_matches_src_dst() {
        let q = create_edge_query("KNOWS");
        assert!(q.contains("MATCH (a {"), "missing src match: {q}");
        assert!(q.contains("(b {"), "missing dst match: {q}");
        assert!(q.contains("$src"), "missing $src param: {q}");
        assert!(q.contains("$dst"), "missing $dst param: {q}");
        assert!(q.contains(":`KNOWS`"), "missing rel type: {q}");
        assert!(q.contains("CREATE (a)-[r:"), "missing CREATE: {q}");
        assert!(q.contains("]->(b)"), "missing directed creation: {q}");
    }

    #[test]
    fn set_edge_properties_query_uses_merge() {
        let q = set_edge_properties_query();
        assert!(q.contains("SET r += $props"), "missing merge SET: {q}");
    }

    #[test]
    fn replace_edge_properties_query_uses_replace() {
        let q = replace_edge_properties_query();
        assert!(q.contains("SET r = $props"), "missing replace SET: {q}");
        assert!(!q.contains("+="), "should use = not +=: {q}");
    }

    #[test]
    fn remove_edge_property_query_interpolates_key() {
        let q = remove_edge_property_query("weight");
        assert!(q.contains("REMOVE r.`weight`"), "got: {q}");
    }

    #[test]
    fn delete_edge_query_structure() {
        let q = delete_edge_query();
        assert!(q.contains("DELETE r"), "missing DELETE: {q}");
        assert!(!q.contains("DETACH"), "edges should not use DETACH DELETE: {q}");
    }

    // ── Edge query consistency ──────────────────────────────────────────────

    #[test]
    fn all_edge_queries_use_directed_pattern() {
        // Every edge query should use directed relationship patterns ()->[]->()
        let queries = [
            get_edge_query(),
            get_edges_batch_query(),
            all_edge_ids_query(),
            set_edge_properties_query(),
            replace_edge_properties_query(),
            delete_edge_query(),
        ];
        for q in &queries {
            assert!(
                q.contains("->()"),
                "edge query missing directed arrow: {q}"
            );
        }
    }

    #[test]
    fn create_edge_query_sanitized_type_no_injection() {
        // Simulate what happens with a malicious type after sanitization
        let sanitized = sanitize_label("KNOWS`; DROP TABLE--");
        let q = create_edge_query(&sanitized);
        assert!(q.contains(":`KNOWS___DROP_TABLE__`"), "got: {q}");
        assert!(!q.contains(";"), "injection not neutralized: {q}");
    }
}
