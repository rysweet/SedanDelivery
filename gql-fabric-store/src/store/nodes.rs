// Node-related Cypher query builders and helpers.
//
// These are pure functions that construct Cypher query strings, making them
// testable without a live Fabric connection.

use crate::{PROP_GRAPH, PROP_LABELS, PROP_NODE_ID, SEQ_LABEL};

/// Cypher: get a single node by graph + node ID.
pub(crate) fn get_node_query() -> String {
    format!("MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) RETURN n LIMIT 1")
}

/// Cypher: UNWIND batch of IDs and OPTIONAL MATCH nodes.
pub(crate) fn get_nodes_batch_query() -> String {
    format!(
        "UNWIND $ids AS target \
         OPTIONAL MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: target}}) \
         RETURN n ORDER BY target"
    )
}

/// Cypher: find nodes by label (sanitized), returning their IDs.
pub(crate) fn find_nodes_by_label_query(label: &str) -> String {
    format!(
        "MATCH (n:`{label}` {{`{PROP_GRAPH}`: $graph}}) RETURN n.`{PROP_NODE_ID}` AS id"
    )
}

/// Cypher: find nodes by label+property with a WHERE predicate.
pub(crate) fn find_nodes_by_label_and_property_query(label: &str, predicate: &str) -> String {
    format!(
        "MATCH (n:`{label}` {{`{PROP_GRAPH}`: $graph}}) \
         WHERE {predicate} \
         RETURN n.`{PROP_NODE_ID}` AS id"
    )
}

/// Cypher: all node IDs in a graph (excluding __SD2Seq).
pub(crate) fn all_node_ids_query() -> String {
    format!(
        "MATCH (n {{`{PROP_GRAPH}`: $graph}}) WHERE NOT n:{SEQ_LABEL} RETURN n.`{PROP_NODE_ID}` AS id"
    )
}

/// Cypher: all label names from _sd2_labels property.
pub(crate) fn all_label_names_query() -> String {
    format!(
        "MATCH (n {{`{PROP_GRAPH}`: $graph}}) WHERE NOT n:{SEQ_LABEL} \
         RETURN n.`{PROP_LABELS}` AS lbl"
    )
}

/// Cypher: CREATE a node with optional labels and props.
pub(crate) fn create_node_query(label_str: &str) -> String {
    format!("CREATE (n{label_str} $props) RETURN n.`{PROP_NODE_ID}` AS id")
}

/// Cypher: SET n += $props (merge properties).
pub(crate) fn set_node_properties_query() -> String {
    format!("MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) SET n += $props")
}

/// Cypher: SET n = $props (replace all properties).
pub(crate) fn replace_node_properties_query() -> String {
    format!("MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) SET n = $props")
}

/// Cypher: REMOVE a single property from a node.
pub(crate) fn remove_node_property_query(key: &str) -> String {
    format!(
        "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) REMOVE n.`{key}`"
    )
}

/// Cypher: SET labels on a node (e.g. `SET n:\`Person\`:\`Employee\``).
pub(crate) fn add_node_labels_query(label_str: &str) -> String {
    format!(
        "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) SET n:{label_str}"
    )
}

/// Cypher: REMOVE labels from a node.
pub(crate) fn remove_node_labels_query(label_str: &str) -> String {
    format!(
        "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) REMOVE n:{label_str}"
    )
}

/// Cypher: check relationship count before non-detach delete.
pub(crate) fn check_node_relationships_query() -> String {
    format!(
        "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) \
         OPTIONAL MATCH (n)-[r]-() \
         RETURN count(r) AS cnt"
    )
}

/// Cypher: DETACH DELETE a node.
pub(crate) fn delete_node_query() -> String {
    format!("MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}}) DETACH DELETE n")
}

/// Cypher: outgoing edges adjacency query.
pub(crate) fn out_edges_query() -> String {
    let eid = crate::PROP_EDGE_ID;
    format!(
        "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}})-[r {{`{PROP_GRAPH}`: $graph}}]->(m) \
         RETURN r.`{eid}` AS eid, m.`{PROP_NODE_ID}` AS nbr, type(r) AS rt"
    )
}

/// Cypher: incoming edges adjacency query.
pub(crate) fn in_edges_query() -> String {
    let eid = crate::PROP_EDGE_ID;
    format!(
        "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}})<-[r {{`{PROP_GRAPH}`: $graph}}]-(m) \
         RETURN r.`{eid}` AS eid, m.`{PROP_NODE_ID}` AS nbr, type(r) AS rt"
    )
}

/// Cypher: outgoing edges filtered by type.
pub(crate) fn out_edges_by_type_query() -> String {
    let eid = crate::PROP_EDGE_ID;
    format!(
        "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}})-[r {{`{PROP_GRAPH}`: $graph}}]->(m) \
         WHERE type(r) IN $types \
         RETURN r.`{eid}` AS eid, m.`{PROP_NODE_ID}` AS nbr, type(r) AS rt"
    )
}

/// Cypher: incoming edges filtered by type.
pub(crate) fn in_edges_by_type_query() -> String {
    let eid = crate::PROP_EDGE_ID;
    format!(
        "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}})<-[r {{`{PROP_GRAPH}`: $graph}}]-(m) \
         WHERE type(r) IN $types \
         RETURN r.`{eid}` AS eid, m.`{PROP_NODE_ID}` AS nbr, type(r) AS rt"
    )
}

/// Cypher: outgoing edge IDs only.
pub(crate) fn out_edge_ids_query() -> String {
    let eid = crate::PROP_EDGE_ID;
    format!(
        "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}})-[r {{`{PROP_GRAPH}`: $graph}}]->() \
         RETURN r.`{eid}` AS id"
    )
}

/// Cypher: incoming edge IDs only.
pub(crate) fn in_edge_ids_query() -> String {
    let eid = crate::PROP_EDGE_ID;
    format!(
        "MATCH (n {{`{PROP_GRAPH}`: $graph, `{PROP_NODE_ID}`: $id}})<-[r {{`{PROP_GRAPH}`: $graph}}]-() \
         RETURN r.`{eid}` AS id"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Node read queries ───────────────────────────────────────────────────

    #[test]
    fn get_node_query_structure() {
        let q = get_node_query();
        assert!(q.starts_with("MATCH (n {"), "got: {q}");
        assert!(q.contains("$graph"), "missing $graph: {q}");
        assert!(q.contains("$id"), "missing $id: {q}");
        assert!(q.contains("RETURN n LIMIT 1"), "missing RETURN: {q}");
    }

    #[test]
    fn get_nodes_batch_query_structure() {
        let q = get_nodes_batch_query();
        assert!(q.contains("UNWIND $ids AS target"), "missing UNWIND: {q}");
        assert!(q.contains("OPTIONAL MATCH"), "missing OPTIONAL MATCH: {q}");
        assert!(q.contains("ORDER BY target"), "missing ORDER BY: {q}");
    }

    #[test]
    fn find_nodes_by_label_query_interpolates_label() {
        let q = find_nodes_by_label_query("Person");
        assert!(q.contains(":`Person`"), "missing label in: {q}");
        assert!(q.contains("$graph"), "missing $graph: {q}");
        assert!(q.contains("RETURN"), "missing RETURN: {q}");
    }

    #[test]
    fn find_nodes_by_label_and_property_query_interpolates() {
        let q = find_nodes_by_label_and_property_query("Person", "n.`age` = $rv");
        assert!(q.contains(":`Person`"), "missing label in: {q}");
        assert!(q.contains("WHERE n.`age` = $rv"), "missing predicate in: {q}");
    }

    #[test]
    fn all_node_ids_query_excludes_seq_label() {
        let q = all_node_ids_query();
        assert!(q.contains("WHERE NOT n:__SD2Seq"), "missing seq filter: {q}");
    }

    #[test]
    fn all_label_names_query_returns_labels_prop() {
        let q = all_label_names_query();
        assert!(q.contains(PROP_LABELS), "missing labels prop: {q}");
        assert!(q.contains("WHERE NOT n:__SD2Seq"), "missing seq filter: {q}");
    }

    // ── Node write queries ──────────────────────────────────────────────────

    #[test]
    fn create_node_query_with_labels() {
        let q = create_node_query(":`Person`:`Employee`");
        assert!(q.contains("CREATE (n:`Person`:`Employee` $props)"), "got: {q}");
        assert!(q.contains("RETURN"), "missing RETURN: {q}");
    }

    #[test]
    fn create_node_query_without_labels() {
        let q = create_node_query("");
        assert!(q.contains("CREATE (n $props)"), "got: {q}");
    }

    #[test]
    fn set_node_properties_query_uses_merge_operator() {
        let q = set_node_properties_query();
        assert!(q.contains("SET n += $props"), "missing merge SET: {q}");
    }

    #[test]
    fn replace_node_properties_query_uses_replace_operator() {
        let q = replace_node_properties_query();
        assert!(q.contains("SET n = $props"), "missing replace SET: {q}");
        // Must NOT contain +=
        assert!(!q.contains("+="), "should use = not +=: {q}");
    }

    #[test]
    fn remove_node_property_query_interpolates_key() {
        let q = remove_node_property_query("age");
        assert!(q.contains("REMOVE n.`age`"), "got: {q}");
    }

    #[test]
    fn add_node_labels_query_sets_labels() {
        let q = add_node_labels_query("`Person`:`Employee`");
        assert!(q.contains("SET n:`Person`:`Employee`"), "got: {q}");
    }

    #[test]
    fn remove_node_labels_query_removes_labels() {
        let q = remove_node_labels_query("`Person`");
        assert!(q.contains("REMOVE n:`Person`"), "got: {q}");
    }

    #[test]
    fn delete_node_query_uses_detach() {
        let q = delete_node_query();
        assert!(q.contains("DETACH DELETE n"), "got: {q}");
    }

    #[test]
    fn check_node_relationships_query_counts() {
        let q = check_node_relationships_query();
        assert!(q.contains("OPTIONAL MATCH (n)-[r]-()"), "got: {q}");
        assert!(q.contains("count(r) AS cnt"), "got: {q}");
    }

    // ── Adjacency queries ───────────────────────────────────────────────────

    #[test]
    fn out_edges_query_direction() {
        let q = out_edges_query();
        assert!(q.contains("]->(m)"), "missing outbound arrow: {q}");
        assert!(q.contains("AS eid"), "missing eid alias: {q}");
        assert!(q.contains("AS nbr"), "missing nbr alias: {q}");
        assert!(q.contains("type(r) AS rt"), "missing type alias: {q}");
    }

    #[test]
    fn in_edges_query_direction() {
        let q = in_edges_query();
        assert!(q.contains("]-(m)"), "missing inbound pattern: {q}");
        assert!(q.contains("<-[r"), "missing inbound arrow: {q}");
    }

    #[test]
    fn out_edges_by_type_query_has_type_filter() {
        let q = out_edges_by_type_query();
        assert!(q.contains("WHERE type(r) IN $types"), "missing type filter: {q}");
        assert!(q.contains("]->(m)"), "missing outbound arrow: {q}");
    }

    #[test]
    fn in_edges_by_type_query_has_type_filter() {
        let q = in_edges_by_type_query();
        assert!(q.contains("WHERE type(r) IN $types"), "missing type filter: {q}");
        assert!(q.contains("]-(m)"), "missing inbound pattern: {q}");
    }

    #[test]
    fn out_edge_ids_query_returns_id_only() {
        let q = out_edge_ids_query();
        assert!(q.contains("RETURN r."), "missing RETURN: {q}");
        assert!(q.contains("AS id"), "missing id alias: {q}");
        assert!(q.contains("}]->()"), "missing directed arrow: {q}");
    }

    #[test]
    fn in_edge_ids_query_returns_id_only() {
        let q = in_edge_ids_query();
        assert!(q.contains("RETURN r."), "missing RETURN: {q}");
        assert!(q.contains("AS id"), "missing id alias: {q}");
        assert!(q.contains("}]-()"), "missing inbound pattern: {q}");
    }
}
