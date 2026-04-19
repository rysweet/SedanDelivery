// Cross-cutting helpers shared by node, edge, and admin operations.
//
// - `refresh_labels_prop`, `next_node_id`, `next_edge_id`: inherent methods on
//   `FabricGraphStore` that issue Cypher to keep metadata in sync or allocate
//   monotonic IDs from the per-graph sequence node.
// - `build_label_str`, `build_set_label_str`: pure label-string builders used
//   when constructing CREATE / SET / REMOVE clauses.

use serde_json::json;

use cypher_core::error::StorageError;
use cypher_core::storage::{EdgeId, NodeId};

use crate::sanitize::sanitize_label;
use crate::{FabricGraphStore, PROP_GRAPH, PROP_LABELS, PROP_NODE_ID, SEQ_LABEL};

// ── Shared inherent helpers ─────────────────────────────────────────────────

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
        let labels_json =
            serde_json::to_string(&labels).expect("Vec<String> serialization is infallible");
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
