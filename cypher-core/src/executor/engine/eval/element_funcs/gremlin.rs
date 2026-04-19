//! Gremlin `T.label`, `T.key`, `T.value` function arms.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::super::QueryExecutor;

impl<'a> QueryExecutor<'a> {
    // Gremlin T.label: returns single label string (not a list)
    pub(in crate::executor::engine::eval) fn eval_func_gremlin_single_label(
        &self,
        graph: &str,
        evaled_args: &[Value],
    ) -> Result<Value, GqlError> {
        if evaled_args.len() != 1 {
            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "gremlin_single_label() expects 1 argument".to_string(),
                actual: format!("{} arguments", evaled_args.len()),
            }));
        }
        match &evaled_args[0] {
            Value::Null => Ok(Value::Null),
            Value::Node(node_id) => match self.cached_get_node(graph, *node_id)? {
                Some(data) => {
                    if let Some(lbl) = data.labels.first() {
                        Ok(Value::Str(lbl.clone().into_boxed_str()))
                    } else {
                        Ok(Value::Str("".to_string().into_boxed_str()))
                    }
                }
                None => Ok(Value::Null),
            },
            Value::Edge(edge_id) => match self.cached_get_edge(graph, *edge_id)? {
                Some(data) => Ok(Value::Str(data.rel_type.into_boxed_str())),
                None => Ok(Value::Null),
            },
            // Property Map: return ~key as label
            Value::Map(m) if m.contains_key("~key") => {
                Ok(m.get("~key").cloned().unwrap_or(Value::Null))
            }
            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Node".to_string(),
                actual: other.type_name().to_string(),
            })),
        }
    }

    // Gremlin T.key: extract ~key from property map
    pub(in crate::executor::engine::eval) fn eval_func_gremlin_property_key(
        &self,
        evaled_args: &[Value],
    ) -> Result<Value, GqlError> {
        if evaled_args.len() != 1 {
            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "gremlin_property_key() expects 1 argument".to_string(),
                actual: format!("{} arguments", evaled_args.len()),
            }));
        }
        match &evaled_args[0] {
            Value::Map(m) if m.contains_key("~key") => {
                Ok(m.get("~key").cloned().unwrap_or(Value::Null))
            }
            Value::Null => Ok(Value::Null),
            _ => Ok(Value::Null),
        }
    }

    // Gremlin T.value: extract ~value from property map
    pub(in crate::executor::engine::eval) fn eval_func_gremlin_property_value(
        &self,
        evaled_args: &[Value],
    ) -> Result<Value, GqlError> {
        if evaled_args.len() != 1 {
            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "gremlin_property_value() expects 1 argument".to_string(),
                actual: format!("{} arguments", evaled_args.len()),
            }));
        }
        match &evaled_args[0] {
            Value::Map(m) if m.contains_key("~value") => {
                Ok(m.get("~value").cloned().unwrap_or(Value::Null))
            }
            Value::Null => Ok(Value::Null),
            _ => Ok(Value::Null),
        }
    }
}
