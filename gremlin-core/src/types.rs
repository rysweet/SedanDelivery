//! Gremlin value types and conversions to/from gql-core's `Value`.

use gql_core::storage::{EdgeId, NodeId};
use gql_core::types::Value;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// The unified value type for Gremlin traversals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GremlinValue {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    List(Vec<GremlinValue>),
    Map(IndexMap<String, GremlinValue>),
    Vertex {
        id: i64,
        label: String,
        properties: IndexMap<String, GremlinValue>,
    },
    Edge {
        id: i64,
        label: String,
        #[serde(rename = "inV")]
        in_v: i64,
        #[serde(rename = "outV")]
        out_v: i64,
        properties: IndexMap<String, GremlinValue>,
    },
    Path {
        labels: Vec<Vec<String>>,
        objects: Vec<GremlinValue>,
    },
}

/// A traverser carries a value through a Gremlin traversal along with bulk
/// count and path history.
#[derive(Debug, Clone, PartialEq)]
pub struct Traverser {
    pub value: GremlinValue,
    pub bulk: u64,
    pub path: Vec<GremlinValue>,
}

impl Traverser {
    pub fn new(value: GremlinValue) -> Self {
        Self {
            value,
            bulk: 1,
            path: Vec::new(),
        }
    }
}

// ── GremlinValue → gql_core::Value ──────────────────────────────────────────

impl From<GremlinValue> for Value {
    fn from(gv: GremlinValue) -> Self {
        match gv {
            GremlinValue::Null => Value::Null,
            GremlinValue::Bool(b) => Value::Bool(b),
            GremlinValue::Integer(i) => Value::Int(i),
            GremlinValue::Float(f) => Value::Float(f),
            GremlinValue::String(s) => Value::Str(s.into_boxed_str()),
            GremlinValue::List(items) => Value::List(items.into_iter().map(Value::from).collect()),
            GremlinValue::Map(entries) => {
                let map: IndexMap<Box<str>, Value> = entries
                    .into_iter()
                    .map(|(k, v)| (k.into_boxed_str(), Value::from(v)))
                    .collect();
                Value::Map(map)
            }
            GremlinValue::Vertex { id, .. } => Value::Node(NodeId(id as u64)),
            GremlinValue::Edge { id, .. } => Value::Edge(EdgeId(id as u64)),
            GremlinValue::Path { objects, .. } => {
                // Best-effort: store as a list since gql-core Path requires NodeId/EdgeId
                Value::List(objects.into_iter().map(Value::from).collect())
            }
        }
    }
}

// ── gql_core::Value → GremlinValue ──────────────────────────────────────────

impl From<Value> for GremlinValue {
    fn from(v: Value) -> Self {
        match v {
            Value::Null => GremlinValue::Null,
            Value::Bool(b) => GremlinValue::Bool(b),
            Value::Int(i) => GremlinValue::Integer(i),
            Value::Float(f) => GremlinValue::Float(f),
            Value::Str(s) => GremlinValue::String(s.into()),
            Value::List(items) => {
                GremlinValue::List(items.into_iter().map(GremlinValue::from).collect())
            }
            Value::Map(entries) => {
                let map: IndexMap<String, GremlinValue> = entries
                    .into_iter()
                    .map(|(k, v)| (k.into(), GremlinValue::from(v)))
                    .collect();
                GremlinValue::Map(map)
            }
            Value::Node(nid) => GremlinValue::Vertex {
                id: nid.0 as i64,
                label: String::new(),
                properties: IndexMap::new(),
            },
            Value::Edge(eid) => GremlinValue::Edge {
                id: eid.0 as i64,
                label: String::new(),
                in_v: 0,
                out_v: 0,
                properties: IndexMap::new(),
            },
            Value::Path { nodes, edges } => {
                let mut objects = Vec::with_capacity(nodes.len() + edges.len());
                for (i, nid) in nodes.iter().enumerate() {
                    objects.push(GremlinValue::Vertex {
                        id: nid.0 as i64,
                        label: String::new(),
                        properties: IndexMap::new(),
                    });
                    if i < edges.len() {
                        objects.push(GremlinValue::Edge {
                            id: edges[i].0 as i64,
                            label: String::new(),
                            in_v: 0,
                            out_v: 0,
                            properties: IndexMap::new(),
                        });
                    }
                }
                let labels = vec![Vec::new(); objects.len()];
                GremlinValue::Path { labels, objects }
            }
            // Temporal/spatial/bytes → string representation
            Value::Bytes(b) => GremlinValue::List(
                b.into_iter()
                    .map(|x| GremlinValue::Integer(x as i64))
                    .collect(),
            ),
            _ => {
                // Temporal and spatial types have no Gremlin equivalent; represent as string.
                GremlinValue::String(format!("{:?}", v))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_null() {
        let gv = GremlinValue::Null;
        let v: Value = gv.clone().into();
        assert!(matches!(v, Value::Null));
        let back: GremlinValue = v.into();
        assert_eq!(back, gv);
    }

    #[test]
    fn round_trip_scalars() {
        // Bool
        let v: Value = GremlinValue::Bool(true).into();
        assert!(matches!(v, Value::Bool(true)));
        assert_eq!(GremlinValue::from(v), GremlinValue::Bool(true));

        // Integer
        let v: Value = GremlinValue::Integer(42).into();
        assert!(matches!(v, Value::Int(42)));
        assert_eq!(GremlinValue::from(v), GremlinValue::Integer(42));

        // Float
        let v: Value = GremlinValue::Float(3.14).into();
        if let Value::Float(f) = &v {
            assert!((f - 3.14).abs() < f64::EPSILON);
        } else {
            panic!("expected Float");
        }

        // String
        let v: Value = GremlinValue::String("hello".into()).into();
        assert_eq!(v, Value::Str("hello".into()));
        assert_eq!(GremlinValue::from(v), GremlinValue::String("hello".into()));
    }

    #[test]
    fn round_trip_list() {
        let gv = GremlinValue::List(vec![
            GremlinValue::Integer(1),
            GremlinValue::String("two".into()),
        ]);
        let v: Value = gv.into();
        if let Value::List(items) = &v {
            assert_eq!(items.len(), 2);
            assert!(matches!(items[0], Value::Int(1)));
        } else {
            panic!("expected List");
        }
    }

    #[test]
    fn round_trip_map() {
        let mut map = IndexMap::new();
        map.insert("key".to_string(), GremlinValue::Integer(99));
        let gv = GremlinValue::Map(map);
        let v: Value = gv.into();
        if let Value::Map(m) = &v {
            assert_eq!(m.len(), 1);
            assert_eq!(*m.get("key").unwrap(), Value::Int(99));
        } else {
            panic!("expected Map");
        }
    }

    #[test]
    fn vertex_to_node_id() {
        let gv = GremlinValue::Vertex {
            id: 7,
            label: "person".into(),
            properties: IndexMap::new(),
        };
        let v: Value = gv.into();
        assert_eq!(v, Value::Node(NodeId(7)));
    }

    #[test]
    fn edge_to_edge_id() {
        let gv = GremlinValue::Edge {
            id: 3,
            label: "knows".into(),
            in_v: 1,
            out_v: 2,
            properties: IndexMap::new(),
        };
        let v: Value = gv.into();
        assert_eq!(v, Value::Edge(EdgeId(3)));
    }
}
