// GraphStore trait implementation for FabricGraphStore.

use std::collections::HashMap;

use serde_json::{json, Value as JValue};

use gql_core::error::StorageError;
use gql_core::storage::{
    validate_graph_name, AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, IndexDescriptor,
    NodeData, NodeId, RangeQuery,
};
use gql_core::types::Value;

use crate::parsing::{rows_to_adj, user_props_to_json};
use crate::sanitize::{range_to_cypher, sanitize_label};
use crate::{
    FabricGraphStore, PROP_DST, PROP_EDGE_ID, PROP_GRAPH, PROP_LABELS, PROP_NODE_ID, PROP_SRC,
    SEQ_LABEL,
};

impl GraphStore for FabricGraphStore {
    // ── Reads: single items ───────────────────────────────────────────────────

    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError> {
        let rows = self.query(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) RETURN n LIMIT 1"
            ),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        rows.first()
            .map(|r| self.row_to_node_data(r))
            .unwrap_or(Ok(None))
    }

    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError> {
        let rows = self.query(
            &format!("MATCH ()-[r {{`{PROP_GRAPH}`: $graph, `{PROP_EDGE_ID}`: $id}}]->() RETURN r LIMIT 1"),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        rows.first()
            .map(|r| self.row_to_edge_data(r))
            .unwrap_or(Ok(None))
    }

    // ── Reads: batch ──────────────────────────────────────────────────────────

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
            &format!(
                "UNWIND $ids AS target \
                 OPTIONAL MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: target}}) \
                 RETURN n ORDER BY target"
            ),
            json!({ "graph": graph, "ids": id_list }),
        )?;
        rows.iter().map(|r| self.row_to_node_data(r)).collect()
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let id_list: JValue = ids.iter().map(|e| e.0).collect::<Vec<_>>().into();
        let rows = self.query(
            &format!(
                "UNWIND $ids AS target \
                 OPTIONAL MATCH ()-[r {{`{PROP_GRAPH}`: $graph, `{PROP_EDGE_ID}`: target}}]->() \
                 RETURN r ORDER BY target"
            ),
            json!({ "graph": graph, "ids": id_list }),
        )?;
        rows.iter().map(|r| self.row_to_edge_data(r)).collect()
    }

    // ── Reads: label/type indexes ─────────────────────────────────────────────

    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError> {
        let label = sanitize_label(label);
        let rows = self.query(
            &format!(
                "MATCH (n:`{label}` {{`{PROP_GRAPH}`: $graph}}) RETURN n.`{PROP_NODE_ID}` AS id"
            ),
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
        let prop = sanitize_label(property); // same rules
        let (predicate, params) = range_to_cypher(&prop, range);
        let mut p = params;
        p["graph"] = json!(graph);
        let rows = self.query(
            &format!(
                "MATCH (n:`{label}` {{`{PROP_GRAPH}`: $graph}}) \
                 WHERE {predicate} \
                 RETURN n.`{PROP_NODE_ID}` AS id"
            ),
            p,
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_u64().map(NodeId))
            .collect::<Vec<_>>())
    }

    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError> {
        let rt = sanitize_label(rel_type);
        let rows = self.query(
            &format!(
                "MATCH ()-[r:`{rt}` {{`{PROP_GRAPH}`: $graph}}]->() RETURN r.`{PROP_EDGE_ID}` AS id"
            ),
            json!({ "graph": graph }),
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_u64().map(EdgeId))
            .collect::<Vec<_>>())
    }

    // ── Reads: adjacency ──────────────────────────────────────────────────────

    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        let rows = self.query(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}})-[r {{`{PROP_GRAPH}`: $graph}}]->(m) \
                 RETURN r.`{PROP_EDGE_ID}` AS eid, m.`{PROP_NODE_ID}` AS nbr, type(r) AS rt"
            ),
            json!({ "graph": graph, "id": node_id.0 }),
        )?;
        rows_to_adj(rows)
    }

    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError> {
        let rows = self.query(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}})<-[r {{`{PROP_GRAPH}`: $graph}}]-(m) \
                 RETURN r.`{PROP_EDGE_ID}` AS eid, m.`{PROP_NODE_ID}` AS nbr, type(r) AS rt"
            ),
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
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}})-[r {{`{PROP_GRAPH}`: $graph}}]->(m) \
                 WHERE type(r) IN $types \
                 RETURN r.`{PROP_EDGE_ID}` AS eid, m.`{PROP_NODE_ID}` AS nbr, type(r) AS rt"
            ),
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
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}})<-[r {{`{PROP_GRAPH}`: $graph}}]-(m) \
                 WHERE type(r) IN $types \
                 RETURN r.`{PROP_EDGE_ID}` AS eid, m.`{PROP_NODE_ID}` AS nbr, type(r) AS rt"
            ),
            json!({ "graph": graph, "id": node_id.0, "types": rt_list }),
        )?;
        rows_to_adj(rows)
    }

    fn out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        let rows = self.query(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}})-[r {{`{PROP_GRAPH}`: $graph}}]->() \
                 RETURN r.`{PROP_EDGE_ID}` AS id"
            ),
            json!({ "graph": graph, "id": node_id.0 }),
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_u64().map(EdgeId))
            .collect::<Vec<_>>())
    }

    fn in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError> {
        let rows = self.query(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}})<-[r {{`{PROP_GRAPH}`: $graph}}]-() \
                 RETURN r.`{PROP_EDGE_ID}` AS id"
            ),
            json!({ "graph": graph, "id": node_id.0 }),
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_u64().map(EdgeId))
            .collect::<Vec<_>>())
    }

    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError> {
        let rows = self.query(
            &format!("MATCH (n {{`{PROP_GRAPH}`: $graph}}) WHERE NOT n:{SEQ_LABEL} RETURN n.`{PROP_NODE_ID}` AS id"),
            json!({ "graph": graph }),
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_u64().map(NodeId))
            .collect::<Vec<_>>())
    }

    fn all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError> {
        let rows = self.query(
            &format!("MATCH ()-[r {{`{PROP_GRAPH}`: $graph}}]->() RETURN r.`{PROP_EDGE_ID}` AS id"),
            json!({ "graph": graph }),
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_u64().map(EdgeId))
            .collect::<Vec<_>>())
    }

    fn all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError> {
        let rows = self.query(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph}}) WHERE NOT n:{SEQ_LABEL} \
                 RETURN n.`{PROP_LABELS}` AS lbl"
            ),
            json!({ "graph": graph }),
        )?;
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

    // ── Writes: create ────────────────────────────────────────────────────────

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: HashMap<String, Value>,
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

        let label_str = if labels.is_empty() {
            String::new()
        } else {
            format!(
                ":`{}`",
                labels
                    .iter()
                    .map(|l| sanitize_label(l))
                    .collect::<Vec<_>>()
                    .join("`:`")
            )
        };
        let labels_json = serde_json::to_string(&labels).unwrap_or_else(|_| "[]".into());

        let mut all_props: HashMap<String, JValue> = user_props_to_json(&properties);
        all_props.insert(PROP_GRAPH.into(), json!(graph));
        all_props.insert(PROP_NODE_ID.into(), json!(id.0));
        all_props.insert(PROP_LABELS.into(), json!(labels_json));

        self.statement(
            &format!("CREATE (n{label_str} $props) RETURN n.`{PROP_NODE_ID}` AS id"),
            json!({ "props": all_props }),
        )?;

        Ok(id)
    }

    fn create_edge(
        &self,
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
            self.next_edge_id(graph)?
        };

        let rt = sanitize_label(&rel_type);
        let mut all_props: HashMap<String, JValue> = user_props_to_json(&properties);
        all_props.insert(PROP_GRAPH.into(), json!(graph));
        all_props.insert(PROP_EDGE_ID.into(), json!(id.0));
        all_props.insert(PROP_SRC.into(), json!(src.0));
        all_props.insert(PROP_DST.into(), json!(dst.0));

        let rows = self.statement(
            &format!(
                "MATCH (a {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $src}}), \
                       (b {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $dst}}) \
                 CREATE (a)-[r:`{rt}` $props]->(b) \
                 RETURN r.`{PROP_EDGE_ID}` AS id"
            ),
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

    // ── Writes: property updates ──────────────────────────────────────────────

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let props = user_props_to_json(&properties);
        self.statement(
            &format!("MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) SET n += $props"),
            json!({ "graph": graph, "id": id.0, "props": props }),
        )?;
        Ok(())
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut props = user_props_to_json(&properties);
        props.insert(PROP_GRAPH.into(), json!(graph));
        props.insert(PROP_NODE_ID.into(), json!(id.0));
        self.statement(
            &format!("MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) SET n = $props"),
            json!({ "graph": graph, "id": id.0, "props": props }),
        )?;
        self.refresh_labels_prop(graph, id)?;
        Ok(())
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let props = user_props_to_json(&properties);
        self.statement(
            &format!("MATCH ()-[r {{`{PROP_GRAPH}`: $graph, `{PROP_EDGE_ID}`: $id}}]->() SET r += $props"),
            json!({ "graph": graph, "id": id.0, "props": props }),
        )?;
        Ok(())
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let edge = self
            .get_edge(graph, id)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        let mut props = user_props_to_json(&properties);
        props.insert(PROP_GRAPH.into(), json!(graph));
        props.insert(PROP_EDGE_ID.into(), json!(id.0));
        props.insert(PROP_SRC.into(), json!(edge.src.0));
        props.insert(PROP_DST.into(), json!(edge.dst.0));
        self.statement(
            &format!(
                "MATCH ()-[r {{`{PROP_GRAPH}`: $graph, `{PROP_EDGE_ID}`: $id}}]->() SET r = $props"
            ),
            json!({ "graph": graph, "id": id.0, "props": props }),
        )?;
        Ok(())
    }

    fn remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError> {
        let key = sanitize_label(key);
        self.statement(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) REMOVE n.`{key}`"
            ),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        Ok(())
    }

    fn remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError> {
        let key = sanitize_label(key);
        self.statement(
            &format!("MATCH ()-[r {{`{PROP_GRAPH}`: $graph, `{PROP_EDGE_ID}`: $id}}]->() REMOVE r.`{key}`"),
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
        let label_str = labels
            .iter()
            .map(|l| format!("`{}`", sanitize_label(l)))
            .collect::<Vec<_>>()
            .join(":");
        if label_str.is_empty() {
            return Ok(());
        }
        self.statement(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) SET n:{label_str}"
            ),
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
        let label_str = labels
            .iter()
            .map(|l| format!("`{}`", sanitize_label(l)))
            .collect::<Vec<_>>()
            .join(":");
        if label_str.is_empty() {
            return Ok(());
        }
        self.statement(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) REMOVE n:{label_str}"
            ),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        self.refresh_labels_prop(graph, id)?;
        Ok(())
    }

    // ── Writes: delete ────────────────────────────────────────────────────────

    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        if !detach {
            let rows = self.query(
                &format!(
                    "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) \
                     OPTIONAL MATCH (n)-[r]-() \
                     RETURN count(r) AS cnt"
                ),
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
            &format!("MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) DETACH DELETE n"),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        Ok(())
    }

    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.statement(
            &format!("MATCH ()-[r {{`{PROP_GRAPH}`: $graph, `{PROP_EDGE_ID}`: $id}}]->() DELETE r"),
            json!({ "graph": graph, "id": id.0 }),
        )?;
        Ok(())
    }

    // ── Index management ──────────────────────────────────────────────────────

    fn list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError> {
        let rows = self.query(
            &format!(
                "MATCH (idx:__SD2Index {{`{PROP_GRAPH}`: $graph}}) \
                 RETURN idx._label AS lbl, idx._property AS prop"
            ),
            json!({ "graph": graph }),
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let lbl = r.first()?.as_str()?.to_string();
                let prop = r.get(1)?.as_str()?.to_string();
                Some(IndexDescriptor {
                    label: lbl,
                    property: prop,
                })
            })
            .collect())
    }

    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        let label_s = sanitize_label(label);
        let prop_s = sanitize_label(property);
        self.statement(
            &format!("CREATE INDEX sd2_{label_s}_{prop_s} IF NOT EXISTS FOR (n:`{label_s}`) ON (n.`{prop_s}`)"),
            json!({}),
        )?;
        self.statement(
            &format!(
                "MERGE (__i:__SD2Index {{`{PROP_GRAPH}`: $graph, _label: $lbl, _property: $prop}})"
            ),
            json!({ "graph": graph, "lbl": label, "prop": property }),
        )?;
        Ok(())
    }

    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        let label_s = sanitize_label(label);
        let prop_s = sanitize_label(property);
        self.statement(
            &format!("DROP INDEX sd2_{label_s}_{prop_s} IF EXISTS"),
            json!({}),
        )
        ?;
        self.statement(
            &format!(
                "MATCH (__i:__SD2Index {{`{PROP_GRAPH}`: $graph, _label: $lbl, _property: $prop}}) DELETE __i"
            ),
            json!({ "graph": graph, "lbl": label, "prop": property }),
        )?;
        Ok(())
    }

    // ── Graph administration ──────────────────────────────────────────────────

    fn create_graph(&self, name: &str) -> Result<(), StorageError> {
        validate_graph_name(name)?;
        self.statement(
            &format!(
                "MERGE (c:{SEQ_LABEL} {{_graph: $graph, _is_root: true}}) \
                 ON CREATE SET c.node_seq = 0, c.edge_seq = 0"
            ),
            json!({ "graph": name }),
        )?;
        Ok(())
    }

    fn delete_graph(&self, name: &str) -> Result<(), StorageError> {
        validate_graph_name(name)?;
        self.statement(
            &format!("MATCH (n {{`{PROP_GRAPH}`: $graph}}) DETACH DELETE n"),
            json!({ "graph": name }),
        )?;
        self.statement(
            &format!("MATCH (c:{SEQ_LABEL} {{_graph: $graph}}) DETACH DELETE c"),
            json!({ "graph": name }),
        )?;
        Ok(())
    }

    fn graph_exists(&self, name: &str) -> Result<bool, StorageError> {
        validate_graph_name(name)?;
        let rows = self.query(
            &format!(
                "MATCH (c:{SEQ_LABEL} {{_graph: $graph, _is_root: true}}) RETURN count(c) AS n"
            ),
            json!({ "graph": name }),
        )?;
        Ok(rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            > 0)
    }

    fn list_graphs(&self) -> Result<Vec<String>, StorageError> {
        let rows = self.query(
            &format!("MATCH (c:{SEQ_LABEL} {{_is_root: true}}) RETURN c._graph AS g"),
            json!({}),
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first()?.as_str().map(|s| s.to_string()))
            .collect())
    }

    fn graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError> {
        let rows = self.query(
            &format!(
                "MATCH (n {{`{PROP_GRAPH}`: $graph}}) WHERE NOT n:{SEQ_LABEL} \
                 UNWIND labels(n) AS lbl \
                 WITH count(DISTINCT n) AS nc, \
                      count(DISTINCT CASE WHEN NOT lbl STARTS WITH '__SD2' THEN lbl END) AS lc \
                 OPTIONAL MATCH ()-[r {{`{PROP_GRAPH}`: $graph}}]->() \
                 WITH nc, lc, count(r) AS ec, count(DISTINCT type(r)) AS rtc \
                 RETURN nc, ec, lc, rtc"
            ),
            json!({ "graph": graph }),
        )?;
        let row = rows.first().cloned().unwrap_or_default();
        Ok(GraphStats {
            node_count: row.first().and_then(|v| v.as_u64()).unwrap_or(0),
            edge_count: row.get(1).and_then(|v| v.as_u64()).unwrap_or(0),
            label_count: row.get(2).and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            rel_type_count: row.get(3).and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ── Private helpers on FabricGraphStore ──────────────────────────────────────

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
        let labels_json = serde_json::to_string(&labels).unwrap_or_else(|_| "[]".into());
        self.statement(
            &format!("MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) SET n.`{PROP_LABELS}` = $lbls"),
            json!({ "graph": graph, "id": id.0, "lbls": labels_json }),
        )?;
        Ok(())
    }
}
