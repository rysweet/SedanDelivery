//! `properties()`, `keys()`, `propertiesMatch()` function arms.

use crate::error::{ExecutionError, GqlError};
use crate::executor::filter::map_key_to_value;
use crate::types::Value;

use super::super::super::QueryExecutor;

impl<'a> QueryExecutor<'a> {
    pub(in crate::executor::engine::eval) fn eval_func_properties_match(
        &self,
        graph: &str,
        evaled_args: &[Value],
    ) -> Result<Value, GqlError> {
        if evaled_args.len() < 2 {
            return Ok(Value::Bool(false));
        }
        let param_map = match &evaled_args[1] {
            Value::Map(m) => m,
            Value::Null => return Ok(Value::Null),
            other => {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Map for propertiesMatch".to_string(),
                    actual: other.type_name().to_string(),
                }));
            }
        };
        let entity_props = match &evaled_args[0] {
            Value::Node(node_id) => match self.cached_get_node(graph, *node_id)? {
                Some(data) => data.properties.clone(),
                None => return Ok(Value::Bool(false)),
            },
            Value::Edge(edge_id) => match self.cached_get_edge(graph, *edge_id)? {
                Some(data) => data.properties.clone(),
                None => return Ok(Value::Bool(false)),
            },
            _ => return Ok(Value::Bool(false)),
        };
        let all_match = param_map.iter().all(|(key, expected)| {
            entity_props
                .get(&**key)
                .map(|v| v == expected)
                .unwrap_or(false)
        });
        Ok(Value::Bool(all_match))
    }

    pub(in crate::executor::engine::eval) fn eval_func_properties(
        &self,
        graph: &str,
        evaled_args: &[Value],
    ) -> Result<Value, GqlError> {
        if evaled_args.len() != 1 {
            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "properties() expects 1 argument".to_string(),
                actual: format!("{} arguments", evaled_args.len()),
            }));
        }
        match &evaled_args[0] {
            Value::Null => Ok(Value::Null),
            Value::Node(node_id) => match self.cached_get_node(graph, *node_id)? {
                Some(data) => {
                    let map: indexmap::IndexMap<Box<str>, Value> = data
                        .properties
                        .into_iter()
                        .map(|(k, v)| (k.into_boxed_str(), v))
                        .collect();
                    Ok(Value::Map(map))
                }
                None => Ok(Value::Map(indexmap::IndexMap::new())),
            },
            Value::Edge(edge_id) => match self.cached_get_edge(graph, *edge_id)? {
                Some(data) => {
                    let map: indexmap::IndexMap<Box<str>, Value> = data
                        .properties
                        .into_iter()
                        .map(|(k, v)| (k.into_boxed_str(), v))
                        .collect();
                    Ok(Value::Map(map))
                }
                None => Ok(Value::Map(indexmap::IndexMap::new())),
            },
            Value::Map(m) => Ok(Value::Map(m.clone())),
            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Node, Relationship, or Map".to_string(),
                actual: other.type_name().to_string(),
            })),
        }
    }

    pub(in crate::executor::engine::eval) fn eval_func_keys(
        &self,
        graph: &str,
        evaled_args: &[Value],
    ) -> Result<Value, GqlError> {
        if evaled_args.len() != 1 {
            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "keys() expects 1 argument".to_string(),
                actual: format!("{} arguments", evaled_args.len()),
            }));
        }
        match &evaled_args[0] {
            Value::Null => Ok(Value::Null),
            Value::Node(node_id) => match self.cached_get_node(graph, *node_id)? {
                Some(data) => Ok(Value::List(
                    data.properties
                        .iter()
                        .filter(|(_, v)| !v.is_null())
                        .map(|(k, _)| Value::Str(k.clone().into_boxed_str()))
                        .collect(),
                )),
                None => Ok(Value::List(vec![])),
            },
            Value::Edge(edge_id) => match self.cached_get_edge(graph, *edge_id)? {
                Some(data) => Ok(Value::List(
                    data.properties
                        .iter()
                        .filter(|(_, v)| !v.is_null())
                        .map(|(k, _)| Value::Str(k.clone().into_boxed_str()))
                        .collect(),
                )),
                None => Ok(Value::List(vec![])),
            },
            Value::Map(m) => Ok(Value::List(m.keys().map(|k| map_key_to_value(k)).collect())),
            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Node, Relationship, or Map".to_string(),
                actual: other.type_name().to_string(),
            })),
        }
    }
}
