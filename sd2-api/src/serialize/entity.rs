//! Wire representations for nodes and edges.

use serde_json::{json, Value as Json};

use cypher_core::storage::{EdgeData, NodeData};

use super::value::props_to_json;

/// Wire representation of a node returned by the API.
pub fn node_to_json(node: &NodeData) -> Json {
    json!({
        "id":         node.id.0.to_string(),
        "labels":     node.labels,
        "properties": props_to_json(&node.properties),
    })
}

/// Wire representation of an edge returned by the API.
pub fn edge_to_json(edge: &EdgeData) -> Json {
    json!({
        "id":         edge.id.0.to_string(),
        "type":       edge.rel_type,
        "source":     edge.src.0.to_string(),
        "target":     edge.dst.0.to_string(),
        "properties": props_to_json(&edge.properties),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cypher_core::storage::{EdgeData, EdgeId, NodeData, NodeId};
    use cypher_core::types::Value;
    use indexmap::IndexMap;
    use serde_json::json;

    #[test]
    fn node_to_json_basic() {
        let mut props = IndexMap::new();
        props.insert("name".to_string(), Value::Str("Bob".into()));
        let node = NodeData {
            id: NodeId(7),
            labels: vec!["Person".to_string(), "Employee".to_string()],
            properties: props,
        };
        let result = node_to_json(&node);
        assert_eq!(result["id"], json!("7"));
        assert_eq!(result["labels"], json!(["Person", "Employee"]));
        assert_eq!(result["properties"]["name"], json!("Bob"));
    }

    #[test]
    fn node_to_json_no_labels_no_props() {
        let node = NodeData {
            id: NodeId(0),
            labels: vec![],
            properties: IndexMap::new(),
        };
        let result = node_to_json(&node);
        assert_eq!(result["id"], json!("0"));
        assert_eq!(result["labels"], json!([]));
        assert_eq!(result["properties"], json!({}));
    }

    #[test]
    fn edge_to_json_basic() {
        let mut props = IndexMap::new();
        props.insert("weight".to_string(), Value::Float(0.75));
        let edge = EdgeData {
            id: EdgeId(42),
            rel_type: "KNOWS".to_string(),
            src: NodeId(1),
            dst: NodeId(2),
            properties: props,
        };
        let result = edge_to_json(&edge);
        assert_eq!(result["id"], json!("42"));
        assert_eq!(result["type"], json!("KNOWS"));
        assert_eq!(result["source"], json!("1"));
        assert_eq!(result["target"], json!("2"));
        assert_eq!(result["properties"]["weight"], json!(0.75));
    }

    #[test]
    fn edge_to_json_no_properties() {
        let edge = EdgeData {
            id: EdgeId(1),
            rel_type: "FOLLOWS".to_string(),
            src: NodeId(10),
            dst: NodeId(20),
            properties: IndexMap::new(),
        };
        let result = edge_to_json(&edge);
        assert_eq!(result["id"], json!("1"));
        assert_eq!(result["type"], json!("FOLLOWS"));
        assert_eq!(result["properties"], json!({}));
    }
}
