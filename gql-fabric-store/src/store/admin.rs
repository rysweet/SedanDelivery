// Graph administration: lifecycle (create/delete/exists/list), stats, and index management.

use serde_json::json;

use gql_core::error::StorageError;
use gql_core::storage::{validate_graph_name, GraphStats, IndexDescriptor};

use crate::sanitize::sanitize_label;
use crate::{FabricGraphStore, PROP_GRAPH, SEQ_LABEL};

// ── Cypher query builders ───────────────────────────────────────────────────

/// Cypher: list indexes stored as __SD2Index nodes.
pub(crate) fn list_indexes_query() -> String {
    format!(
        "MATCH (idx:__SD2Index {{`{PROP_GRAPH}`: $graph}}) \
         RETURN idx._label AS lbl, idx._property AS prop"
    )
}

/// Cypher: CREATE INDEX with a native Fabric/Neo4j index.
pub(crate) fn create_index_native_query(label_s: &str, prop_s: &str) -> String {
    format!(
        "CREATE INDEX sd2_{label_s}_{prop_s} IF NOT EXISTS FOR (n:`{label_s}`) ON (n.`{prop_s}`)"
    )
}

/// Cypher: MERGE the __SD2Index metadata node for a created index.
pub(crate) fn create_index_meta_query() -> String {
    format!("MERGE (__i:__SD2Index {{`{PROP_GRAPH}`: $graph, _label: $lbl, _property: $prop}})")
}

/// Cypher: DROP INDEX by name.
pub(crate) fn drop_index_native_query(label_s: &str, prop_s: &str) -> String {
    format!("DROP INDEX sd2_{label_s}_{prop_s} IF EXISTS")
}

/// Cypher: DELETE the __SD2Index metadata node for a dropped index.
pub(crate) fn drop_index_meta_query() -> String {
    format!(
        "MATCH (__i:__SD2Index {{`{PROP_GRAPH}`: $graph, _label: $lbl, _property: $prop}}) DELETE __i"
    )
}

/// Cypher: MERGE the sequence counter node for a new graph.
pub(crate) fn create_graph_query() -> String {
    format!(
        "MERGE (c:{SEQ_LABEL} {{_graph: $graph, _is_root: true}}) \
         ON CREATE SET c.node_seq = 0, c.edge_seq = 0"
    )
}

/// Cypher: delete all graph-namespaced nodes.
pub(crate) fn delete_graph_data_query() -> String {
    format!("MATCH (n {{`{PROP_GRAPH}`: $graph}}) DETACH DELETE n")
}

/// Cypher: delete the sequence counter node.
pub(crate) fn delete_graph_seq_query() -> String {
    format!("MATCH (c:{SEQ_LABEL} {{_graph: $graph}}) DETACH DELETE c")
}

/// Cypher: check if graph exists via its root sequence node.
pub(crate) fn graph_exists_query() -> String {
    format!("MATCH (c:{SEQ_LABEL} {{_graph: $graph, _is_root: true}}) RETURN count(c) AS n")
}

/// Cypher: list all graph names.
pub(crate) fn list_graphs_query() -> String {
    format!("MATCH (c:{SEQ_LABEL} {{_is_root: true}}) RETURN c._graph AS g")
}

/// Cypher: compute graph statistics.
pub(crate) fn graph_stats_query() -> String {
    format!(
        "MATCH (n {{`{PROP_GRAPH}`: $graph}}) WHERE NOT n:{SEQ_LABEL} \
         UNWIND labels(n) AS lbl \
         WITH count(DISTINCT n) AS nc, \
              count(DISTINCT CASE WHEN NOT lbl STARTS WITH '__SD2' THEN lbl END) AS lc \
         OPTIONAL MATCH ()-[r {{`{PROP_GRAPH}`: $graph}}]->() \
         WITH nc, lc, count(r) AS ec, count(DISTINCT type(r)) AS rtc \
         RETURN nc, ec, lc, rtc"
    )
}

// ── Implementation functions (called by GraphStore trait impl) ──────────────

pub(super) fn list_indexes_impl(
    store: &FabricGraphStore,
    graph: &str,
) -> Result<Vec<IndexDescriptor>, StorageError> {
    let rows = store.query(&list_indexes_query(), json!({ "graph": graph }))?;
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

pub(super) fn create_index_impl(
    store: &FabricGraphStore,
    graph: &str,
    label: &str,
    property: &str,
) -> Result<(), StorageError> {
    let label_s = sanitize_label(label);
    if label_s.is_empty() {
        tracing::warn!(label = %label, "skipping create_index: empty label after sanitization");
        return Ok(());
    }
    let prop_s = sanitize_label(property);
    if prop_s.is_empty() {
        tracing::warn!(property = %property, "skipping create_index: empty property after sanitization");
        return Ok(());
    }
    store.statement(&create_index_native_query(&label_s, &prop_s), json!({}))?;
    store.statement(
        &create_index_meta_query(),
        json!({ "graph": graph, "lbl": label, "prop": property }),
    )?;
    Ok(())
}

pub(super) fn drop_index_impl(
    store: &FabricGraphStore,
    graph: &str,
    label: &str,
    property: &str,
) -> Result<(), StorageError> {
    let label_s = sanitize_label(label);
    if label_s.is_empty() {
        tracing::warn!(label = %label, "skipping drop_index: empty label after sanitization");
        return Ok(());
    }
    let prop_s = sanitize_label(property);
    if prop_s.is_empty() {
        tracing::warn!(property = %property, "skipping drop_index: empty property after sanitization");
        return Ok(());
    }
    store.statement(&drop_index_native_query(&label_s, &prop_s), json!({}))?;
    store.statement(
        &drop_index_meta_query(),
        json!({ "graph": graph, "lbl": label, "prop": property }),
    )?;
    Ok(())
}

pub(super) fn create_graph_impl(store: &FabricGraphStore, name: &str) -> Result<(), StorageError> {
    validate_graph_name(name)?;
    store.statement(&create_graph_query(), json!({ "graph": name }))?;
    Ok(())
}

pub(super) fn delete_graph_impl(store: &FabricGraphStore, name: &str) -> Result<(), StorageError> {
    validate_graph_name(name)?;
    store.statement(&delete_graph_data_query(), json!({ "graph": name }))?;
    store.statement(&delete_graph_seq_query(), json!({ "graph": name }))?;
    Ok(())
}

pub(super) fn graph_exists_impl(
    store: &FabricGraphStore,
    name: &str,
) -> Result<bool, StorageError> {
    validate_graph_name(name)?;
    let rows = store.query(&graph_exists_query(), json!({ "graph": name }))?;
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        > 0)
}

pub(super) fn list_graphs_impl(store: &FabricGraphStore) -> Result<Vec<String>, StorageError> {
    let rows = store.query(&list_graphs_query(), json!({}))?;
    Ok(rows
        .iter()
        .filter_map(|r| r.first()?.as_str().map(|s| s.to_string()))
        .collect())
}

pub(super) fn graph_stats_impl(
    store: &FabricGraphStore,
    graph: &str,
) -> Result<GraphStats, StorageError> {
    let rows = store.query(&graph_stats_query(), json!({ "graph": graph }))?;
    let row = rows.first().ok_or_else(|| StorageError::Backend {
        msg: format!("graph_stats: Fabric returned no rows for graph '{graph}'"),
    })?;
    let parse_stat = |idx: usize, name: &str| -> Result<u64, StorageError> {
        row.get(idx)
            .and_then(|v| v.as_u64())
            .ok_or_else(|| StorageError::Backend {
                msg: format!(
                    "graph_stats: unparseable {name} at index {idx} in Fabric response: {:?}",
                    row.get(idx)
                ),
            })
    };
    let node_count = parse_stat(0, "node_count")?;
    let edge_count = parse_stat(1, "edge_count")?;
    let label_count = parse_stat(2, "label_count")? as u32;
    let rel_type_count = parse_stat(3, "rel_type_count")? as u32;
    Ok(GraphStats {
        node_count,
        edge_count,
        label_count,
        rel_type_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Index queries ───────────────────────────────────────────────────────

    #[test]
    fn list_indexes_query_matches_sd2index_label() {
        let q = list_indexes_query();
        assert!(q.contains(":__SD2Index"), "missing __SD2Index label: {q}");
        assert!(q.contains("$graph"), "missing $graph param: {q}");
        assert!(q.contains("AS lbl"), "missing lbl alias: {q}");
        assert!(q.contains("AS prop"), "missing prop alias: {q}");
    }

    #[test]
    fn create_index_native_query_structure() {
        let q = create_index_native_query("Person", "name");
        assert!(
            q.contains("CREATE INDEX sd2_Person_name IF NOT EXISTS"),
            "got: {q}"
        );
        assert!(q.contains("FOR (n:`Person`)"), "missing label: {q}");
        assert!(q.contains("ON (n.`name`)"), "missing property: {q}");
    }

    #[test]
    fn create_index_meta_query_uses_merge() {
        let q = create_index_meta_query();
        assert!(q.contains("MERGE"), "missing MERGE: {q}");
        assert!(q.contains(":__SD2Index"), "missing label: {q}");
        assert!(q.contains("$graph"), "missing $graph: {q}");
        assert!(q.contains("$lbl"), "missing $lbl: {q}");
        assert!(q.contains("$prop"), "missing $prop: {q}");
    }

    #[test]
    fn drop_index_native_query_structure() {
        let q = drop_index_native_query("Person", "name");
        assert!(
            q.contains("DROP INDEX sd2_Person_name IF EXISTS"),
            "got: {q}"
        );
    }

    #[test]
    fn drop_index_meta_query_deletes_node() {
        let q = drop_index_meta_query();
        assert!(q.contains("MATCH"), "missing MATCH: {q}");
        assert!(q.contains(":__SD2Index"), "missing label: {q}");
        assert!(q.contains("DELETE __i"), "missing DELETE: {q}");
    }

    #[test]
    fn index_query_injection_safety() {
        // Sanitized labels should prevent injection
        let label = sanitize_label("Person`; DROP TABLE--");
        let prop = sanitize_label("name`; --");
        let q = create_index_native_query(&label, &prop);
        assert!(!q.contains(";"), "injection not neutralized: {q}");
        assert!(!q.contains("DROP TABLE"), "injection not neutralized: {q}");
    }

    // ── Graph lifecycle queries ─────────────────────────────────────────────

    #[test]
    fn create_graph_query_initializes_sequences() {
        let q = create_graph_query();
        assert!(q.contains("MERGE"), "missing MERGE: {q}");
        assert!(q.contains(SEQ_LABEL), "missing seq label: {q}");
        assert!(q.contains("_is_root: true"), "missing _is_root: {q}");
        assert!(q.contains("node_seq = 0"), "missing node_seq init: {q}");
        assert!(q.contains("edge_seq = 0"), "missing edge_seq init: {q}");
    }

    #[test]
    fn delete_graph_data_query_uses_detach() {
        let q = delete_graph_data_query();
        assert!(q.contains("DETACH DELETE n"), "got: {q}");
        assert!(q.contains(PROP_GRAPH), "missing graph filter: {q}");
    }

    #[test]
    fn delete_graph_seq_query_removes_counter() {
        let q = delete_graph_seq_query();
        assert!(q.contains(SEQ_LABEL), "missing seq label: {q}");
        assert!(q.contains("DETACH DELETE c"), "got: {q}");
    }

    #[test]
    fn graph_exists_query_counts_root_nodes() {
        let q = graph_exists_query();
        assert!(q.contains(SEQ_LABEL), "missing seq label: {q}");
        assert!(q.contains("_is_root: true"), "missing root filter: {q}");
        assert!(q.contains("count(c) AS n"), "missing count: {q}");
    }

    #[test]
    fn list_graphs_query_returns_graph_names() {
        let q = list_graphs_query();
        assert!(q.contains(SEQ_LABEL), "missing seq label: {q}");
        assert!(q.contains("_is_root: true"), "missing root filter: {q}");
        assert!(q.contains("c._graph AS g"), "missing graph return: {q}");
    }

    #[test]
    fn graph_stats_query_computes_four_metrics() {
        let q = graph_stats_query();
        assert!(q.contains("RETURN nc, ec, lc, rtc"), "missing metrics: {q}");
        assert!(
            q.contains("count(DISTINCT n) AS nc"),
            "missing node count: {q}"
        );
        assert!(q.contains("count(r) AS ec"), "missing edge count: {q}");
        assert!(
            q.contains("WHERE NOT n:__SD2Seq"),
            "missing seq filter: {q}"
        );
    }

    #[test]
    fn graph_stats_query_filters_internal_labels() {
        let q = graph_stats_query();
        assert!(
            q.contains("NOT lbl STARTS WITH '__SD2'"),
            "missing internal label filter: {q}"
        );
    }

    // ── Delete graph pair (data + seq) consistency ──────────────────────────

    #[test]
    fn delete_graph_queries_use_same_param_name() {
        let q1 = delete_graph_data_query();
        let q2 = delete_graph_seq_query();
        assert!(q1.contains("$graph"), "data query missing $graph: {q1}");
        assert!(q2.contains("$graph"), "seq query missing $graph: {q2}");
    }
}
