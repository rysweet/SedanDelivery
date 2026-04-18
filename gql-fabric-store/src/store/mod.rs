// GraphStore trait implementation for FabricGraphStore, decomposed into focused modules.
//
// Rust requires trait impls to live in a single block, so the `impl GraphStore`
// is here.  The actual logic is organized as:
//
// - nodes.rs:  Cypher query builders for node operations + tests
// - edges.rs:  Cypher query builders + impl functions for edge operations + tests
// - admin.rs:  Cypher query builders + impl functions for admin/index operations + tests
//
// Node operations are inlined in the trait impl (they use query builders from nodes.rs).
// Edge and admin operations delegate to `*_impl` functions in their respective modules.

pub(crate) mod admin;
pub(crate) mod edges;
pub(crate) mod nodes;

use indexmap::IndexMap;
use std::collections::HashMap;

use serde_json::{json, Value as JValue};

use cypher_core::error::StorageError;
use cypher_core::storage::{
    AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor, NodeData, NodeId,
    RangeQuery,
};
use cypher_core::types::Value;

use crate::parsing::{rows_to_adj, user_props_to_json};
use crate::sanitize::{range_to_cypher, sanitize_label};
use crate::{FabricGraphStore, PROP_GRAPH, PROP_LABELS, PROP_NODE_ID, SEQ_LABEL};

// ── Shared helpers on FabricGraphStore ──────────────────────────────────────

impl FabricGraphStore {
    /// After adding/removing labels via SET/REMOVE, refresh the `_sd2_labels` property
    /// so our metadata stays in sync with Fabric's label storage.
    pub(crate) fn refresh_labels_prop(&self, graph: &str, id: NodeId) -> Result<(), StorageError> {
        let rows = self.query(
            &format!("MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) RETURN labels(n) AS lbls"),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        let labels: Vec<String> = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|l| l.as_str())
                    .filter(|l| !l.starts_with("__SD2"))
                    .map(|l| l.to_string())
                    .collect()
            })
            .unwrap_or_default();
        // serde_json::to_string on Vec<String> is infallible (no NaN, no
        // non-UTF8 keys possible), so an Err here would indicate a bug in
        // serde_json itself — propagating with .expect is correct.
        let labels_json = serde_json::to_string(&labels)
            .expect("Vec<String> serialization is infallible");
        self.statement(
            &format!("MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) SET n.`{PROP_LABELS}` = $lbls"),
            json!({ "graph": graph, "id": id.0, "lbls": labels_json }),
        )?;
        Ok(())
    }

    pub(crate) fn next_node_id(&self, graph: &str) -> Result<NodeId, StorageError> {
        let rows = self.statement(
            &format!(
                "MERGE (c:{SEQ_LABEL} {{_graph: $graph}}) \
                 ON CREATE SET c.node_seq = 0 \
                 SET c.node_seq = coalesce(c.node_seq, 0) + 1 \
                 RETURN c.node_seq AS id"
            ),
            json!({ "graph": graph }),
        )?;
        let raw = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.as_u64())
            .ok_or_else(|| StorageError::Backend {
                msg: "no node_seq returned".into(),
            })?;
        Ok(NodeId(raw))
    }

    pub(crate) fn next_edge_id(&self, graph: &str) -> Result<EdgeId, StorageError> {
        let rows = self.statement(
            &format!(
                "MERGE (c:{SEQ_LABEL} {{_graph: $graph}}) \
                 ON CREATE SET c.edge_seq = 0 \
                 SET c.edge_seq = coalesce(c.edge_seq, 0) + 1 \
                 RETURN c.edge_seq AS id"
            ),
            json!({ "graph": graph }),
        )?;
        let raw = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.as_u64())
            .ok_or_else(|| StorageError::Backend {
                msg: "no edge_seq returned".into(),
            })?;
        Ok(EdgeId(raw))
    }
}

// ── Label string builders (pure, testable) ──────────────────────────────────

/// Build a label string for Cypher CREATE from raw labels.
/// Returns empty string if no valid labels remain.
/// Example output: `:`Person`:`Employee``
pub(crate) fn build_label_str(labels: &[String]) -> String {
    let sanitized: Vec<String> = labels
        .iter()
        .filter_map(|l| {
            let s = sanitize_label(l);
            if s.is_empty() {
                tracing::warn!(label = %l, "skipping empty label after sanitization in create_node");
                None
            } else {
                Some(s)
            }
        })
        .collect();
    if sanitized.is_empty() {
        String::new()
    } else {
        format!(":`{}`", sanitized.join("`:`"))
    }
}

/// Build a backtick-quoted colon-separated label string for SET/REMOVE.
/// Returns empty string if no valid labels remain.
/// Example output: `` `Person`:`Employee` ``
pub(crate) fn build_set_label_str(labels: &[String]) -> String {
    labels
        .iter()
        .filter_map(|l| {
            let s = sanitize_label(l);
            if s.is_empty() {
                tracing::warn!(label = %l, "skipping empty label after sanitization");
                None
            } else {
                Some(format!("`{}`", s))
            }
        })
        .collect::<Vec<_>>()
        .join(":")
}

// ── GraphStore trait implementation ─────────────────────────────────────────

impl GraphStore for FabricGraphStore {
    // ── Node reads ──────────────────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        let rows = self.query(
            &nodes::get_node_query(),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        rows.first()
            .map(|r| self.row_to_node_data(r))
            .unwrap_or(Ok(None))
    }

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let id_list: JValue = ids.iter().map(|n| n.0).collect::<Vec<_>>().into();
        let rows = self.query(
            &nodes::get_nodes_batch_query(),
            json!({ "graph": graph, "ids": id_list }),
        )?;
        rows.iter().map(|r| self.row_to_node_data(r)).collect()
    }

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        let sanitized = sanitize_label(label);
        if sanitized.is_empty() {
            return Err(StorageError::Backend {
                msg: format!(
                    "invalid label: sanitize_label produced empty string from {:?}",
                    label
                ),
            });
        }
        let rows = self.query(
            &nodes::find_nodes_by_label_query(&sanitized),
            json!({ "graph": graph }),
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_u64().map(NodeId))
            .collect::<Vec<_>>())
    }

    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        let label = sanitize_label(label);
        let prop = sanitize_label(property);
        let (predicate, params) = range_to_cypher(&prop, range);
        let mut p = params;
        p["graph"] = json!(graph);
        let rows = self.query(
            &nodes::find_nodes_by_label_and_property_query(&label, &predicate),
            p,
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_u64().map(NodeId))
            .collect::<Vec<_>>())
    }

    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        let rows = self.query(&nodes::all_node_ids_query(), json!({ "graph": graph }))?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_u64().map(NodeId))
            .collect::<Vec<_>>())
    }

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        let rows = self.query(&nodes::all_label_names_query(), json!({ "graph": graph }))?;
        let mut seen = std::collections::HashSet::new();
        for row in &rows {
            if let Some(s) = row.first().and_then(|v| v.as_str()) {
                if let Ok(labels) = serde_json::from_str::<Vec<String>>(s) {
                    for l in labels {
                        seen.insert(l);
                    }
                }
            }
        }
        Ok(seen.into_iter().collect())
    }

    // ── Adjacency reads ─────────────────────────────────────────────────────

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        let rows = self.query(
            &nodes::out_edges_query(),
            json!({ "graph": graph, "id": node_id.0 }),
        )?;
        rows_to_adj(rows)
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        let rows = self.query(
            &nodes::in_edges_query(),
            json!({ "graph": graph, "id": node_id.0 }),
        )?;
        rows_to_adj(rows)
    }

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        if rel_types.is_empty() {
            return self.out_edges(graph, node_id);
        }
        let rt_list: JValue = rel_types
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into();
        let rows = self.query(
            &nodes::out_edges_by_type_query(),
            json!({ "graph": graph, "id": node_id.0, "types": rt_list }),
        )?;
        rows_to_adj(rows)
    }

    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        if rel_types.is_empty() {
            return self.in_edges(graph, node_id);
        }
        let rt_list: JValue = rel_types
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into();
        let rows = self.query(
            &nodes::in_edges_by_type_query(),
            json!({ "graph": graph, "id": node_id.0, "types": rt_list }),
        )?;
        rows_to_adj(rows)
    }

    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        let rows = self.query(
            &nodes::out_edge_ids_query(),
            json!({ "graph": graph, "id": node_id.0 }),
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_u64().map(EdgeId))
            .collect::<Vec<_>>())
    }

    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        let rows = self.query(
            &nodes::in_edge_ids_query(),
            json!({ "graph": graph, "id": node_id.0 }),
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_u64().map(EdgeId))
            .collect::<Vec<_>>())
    }

    // ── Node writes ─────────────────────────────────────────────────────────

    fn create_node(
        &self,
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
            self.next_node_id(graph)?
        };

        let label_str = build_label_str(&labels);
        // See comment on the other call site — Vec<String> serialization
        // cannot fail; previously this silently wrote "[]" on an impossible
        // error, which would corrupt label data if it ever did fire.
        let labels_json = serde_json::to_string(&labels)
            .expect("Vec<String> serialization is infallible");

        let mut all_props: HashMap<String, JValue> = user_props_to_json(&properties);
        all_props.insert(PROP_GRAPH.into(), json!(graph));
        all_props.insert(PROP_NODE_ID.into(), json!(id.0));
        all_props.insert(PROP_LABELS.into(), json!(labels_json));

        self.statement(
            &nodes::create_node_query(&label_str),
            json!({ "props": all_props }),
        )?;

        Ok(id)
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        let props = user_props_to_json(&properties);
        self.statement(
            &nodes::set_node_properties_query(),
            json!({ "graph": graph, "id": id.0, "props": props }),
        )?;
        Ok(())
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut props = user_props_to_json(&properties);
        props.insert(PROP_GRAPH.into(), json!(graph));
        props.insert(PROP_NODE_ID.into(), json!(id.0));
        self.statement(
            &nodes::replace_node_properties_query(),
            json!({ "graph": graph, "id": id.0, "props": props }),
        )?;
        self.refresh_labels_prop(graph, id)?;
        Ok(())
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        let key = sanitize_label(key);
        self.statement(
            &nodes::remove_node_property_query(&key),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        Ok(())
    }

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let label_str = build_set_label_str(labels);
        if label_str.is_empty() {
            return Ok(());
        }
        self.statement(
            &nodes::add_node_labels_query(&label_str),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        self.refresh_labels_prop(graph, id)?;
        Ok(())
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        let label_str = build_set_label_str(labels);
        if label_str.is_empty() {
            return Ok(());
        }
        self.statement(
            &nodes::remove_node_labels_query(&label_str),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        self.refresh_labels_prop(graph, id)?;
        Ok(())
    }

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        if !detach {
            let rows = self.query(
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
        self.statement(
            &nodes::delete_node_query(),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        Ok(())
    }

    // ── Edge operations (delegated to edges.rs) ─────────────────────────────

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        edges::get_edge_impl(self, graph, id)
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        edges::get_edges_batch_impl(self, graph, ids)
    }

    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
        edges::find_edges_by_type_impl(self, graph, rel_type)
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        edges::all_edge_ids_impl(self, graph)
    }

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        edges::create_edge_impl(self, graph, src, dst, rel_type, properties)
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        edges::set_edge_properties_impl(self, graph, id, properties)
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        edges::replace_edge_properties_impl(self, graph, id, properties)
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        edges::remove_edge_property_impl(self, graph, id, key)
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        edges::delete_edge_impl(self, graph, id)
    }

    // ── Index management (delegated to admin.rs) ────────────────────────────

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        admin::list_indexes_impl(self, graph)
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        admin::create_index_impl(self, graph, label, property)
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        admin::drop_index_impl(self, graph, label, property)
    }

    // ── Graph administration (delegated to admin.rs) ────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        admin::create_graph_impl(self, name)
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        admin::delete_graph_impl(self, name)
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        admin::graph_exists_impl(self, name)
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        admin::list_graphs_impl(self)
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        admin::graph_stats_impl(self, graph)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── build_label_str ─────────────────────────────────────────────────────

    #[test]
    fn build_label_str_single_label() {
        let result = build_label_str(&["Person".to_string()]);
        assert_eq!(result, ":`Person`");
    }

    #[test]
    fn build_label_str_multiple_labels() {
        let result = build_label_str(&["Person".to_string(), "Employee".to_string()]);
        assert_eq!(result, ":`Person`:`Employee`");
    }

    #[test]
    fn build_label_str_empty_vec() {
        let result = build_label_str(&[]);
        assert_eq!(result, "");
    }

    #[test]
    fn build_label_str_sanitizes_special_chars() {
        let result = build_label_str(&["my-label".to_string()]);
        assert_eq!(result, ":`my_label`");
    }

    #[test]
    fn build_label_str_filters_empty_input() {
        let result = build_label_str(&["".to_string(), "Valid".to_string()]);
        assert_eq!(result, ":`Valid`");
    }

    #[test]
    fn build_label_str_injection_neutralized() {
        let result = build_label_str(&["'; DROP TABLE--".to_string()]);
        assert_eq!(result, ":`___DROP_TABLE__`");
    }

    // ── build_set_label_str ─────────────────────────────────────────────────

    #[test]
    fn build_set_label_str_single() {
        let result = build_set_label_str(&["Person".to_string()]);
        assert_eq!(result, "`Person`");
    }

    #[test]
    fn build_set_label_str_multiple() {
        let result = build_set_label_str(&["A".to_string(), "B".to_string()]);
        assert_eq!(result, "`A`:`B`");
    }

    #[test]
    fn build_set_label_str_empty_input() {
        let result = build_set_label_str(&[]);
        assert_eq!(result, "");
    }

    #[test]
    fn build_set_label_str_sanitizes() {
        let result = build_set_label_str(&["has space".to_string()]);
        assert_eq!(result, "`has_space`");
    }

    #[test]
    fn build_set_label_str_filters_empty() {
        let result = build_set_label_str(&["".to_string()]);
        assert_eq!(result, "");
    }
}
