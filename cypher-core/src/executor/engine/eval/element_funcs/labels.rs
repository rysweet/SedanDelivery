//! `labels()` and `hasLabel()` function arms.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::super::QueryExecutor;

impl<'a> QueryExecutor<'a> {
    pub(in crate::executor::engine::eval) fn eval_func_has_label(
        &self,
        graph: &str,
        evaled_args: &[Value],
    ) -> Result<Value, GqlError> {
        if evaled_args.len() < 2 {
            return Ok(Value::Bool(false));
        }
        let label_name = match &evaled_args[1] {
            Value::Str(s) => s.as_ref().to_string(),
            _ => return Ok(Value::Bool(false)),
        };
        match &evaled_args[0] {
            Value::Node(node_id) => match self.cached_get_node(graph, *node_id)? {
                Some(data) => Ok(Value::Bool(data.labels.iter().any(|l| l == &label_name))),
                None => Ok(Value::Bool(false)),
            },
            _ => Ok(Value::Bool(false)),
        }
    }

    pub(in crate::executor::engine::eval) fn eval_func_labels(
        &self,
        graph: &str,
        evaled_args: &[Value],
    ) -> Result<Value, GqlError> {
        if evaled_args.len() != 1 {
            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "labels() expects 1 argument".to_string(),
                actual: format!("{} arguments", evaled_args.len()),
            }));
        }
        match &evaled_args[0] {
            Value::Null => Ok(Value::Null),
            Value::Node(node_id) => match self.cached_get_node(graph, *node_id)? {
                Some(data) => Ok(Value::List(
                    data.labels
                        .iter()
                        .map(|l| Value::Str(l.clone().into_boxed_str()))
                        .collect(),
                )),
                None => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "accessible Node".to_string(),
                    actual: "deleted or missing Node".to_string(),
                })),
            },
            Value::Edge(edge_id) => match self.cached_get_edge(graph, *edge_id)? {
                Some(data) => Ok(Value::List(vec![Value::Str(
                    data.rel_type.into_boxed_str(),
                )])),
                None => Ok(Value::List(vec![])),
            },
            // Property Map from properties(): key() extracts ~key
            Value::Map(m) if m.contains_key("~key") => {
                let key = m.get("~key").cloned().unwrap_or(Value::Null);
                Ok(key)
            }
            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Node".to_string(),
                actual: other.type_name().to_string(),
            })),
        }
    }
}
