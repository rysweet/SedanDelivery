//! `type()` and `id()` function arms.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::super::QueryExecutor;

impl<'a> QueryExecutor<'a> {
    pub(in crate::executor::engine::eval) fn eval_func_type(
        &self,
        graph: &str,
        evaled_args: &[Value],
    ) -> Result<Value, GqlError> {
        if evaled_args.len() != 1 {
            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "type() expects 1 argument".to_string(),
                actual: format!("{} arguments", evaled_args.len()),
            }));
        }
        match &evaled_args[0] {
            Value::Null => Ok(Value::Null),
            Value::Edge(edge_id) => match self.cached_get_edge(graph, *edge_id)? {
                Some(data) => Ok(Value::Str(data.rel_type.into_boxed_str())),
                None => {
                    let cached = self
                        .deleted_edge_types
                        .borrow()
                        .get(&edge_id.0)
                        .map(|s| Value::Str(s.clone().into_boxed_str()));
                    Ok(cached.unwrap_or(Value::Null))
                }
            },
            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Relationship".to_string(),
                actual: other.type_name().to_string(),
            })),
        }
    }

    pub(in crate::executor::engine::eval) fn eval_func_id(
        &self,
        evaled_args: &[Value],
    ) -> Result<Value, GqlError> {
        if evaled_args.len() != 1 {
            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "id() expects 1 argument".to_string(),
                actual: format!("{} arguments", evaled_args.len()),
            }));
        }
        match &evaled_args[0] {
            Value::Null => Ok(Value::Null),
            Value::Node(node_id) => Ok(Value::Int(node_id.0 as i64)),
            Value::Edge(edge_id) => Ok(Value::Int(edge_id.0 as i64)),
            // Property maps carry ~id for property identity.
            Value::Map(m) if m.contains_key("~id") => {
                Ok(m.get("~id").cloned().unwrap_or(Value::Null))
            }
            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Node or Relationship".to_string(),
                actual: other.type_name().to_string(),
            })),
        }
    }
}
