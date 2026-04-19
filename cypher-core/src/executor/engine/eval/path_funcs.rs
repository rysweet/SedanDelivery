//! Path/edge-related function handlers split out of `eval`:
//! `nodes`, `relationships`, `length`, `startNode`, `endNode`,
//! gremlin `bothV`, `otherV`.

use crate::ast::Expr;
use crate::error::{ExecutionError, GqlError};
use crate::executor::filter::{eval_expr, Row as FilterRow};
use crate::types::Value;

use super::super::helpers::value_to_expr;
use super::super::QueryExecutor;

impl<'a> QueryExecutor<'a> {
    pub(super) fn eval_func_nodes(&self, evaled_args: &[Value]) -> Result<Value, GqlError> {
        if evaled_args.len() != 1 {
            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "nodes() expects 1 argument".to_string(),
                actual: format!("{} arguments", evaled_args.len()),
            }));
        }
        match &evaled_args[0] {
            Value::Null => Ok(Value::Null),
            Value::Path { nodes, .. } => {
                Ok(Value::List(nodes.iter().map(|n| Value::Node(*n)).collect()))
            }
            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Path".to_string(),
                actual: other.type_name().to_string(),
            })),
        }
    }

    pub(super) fn eval_func_relationships(&self, evaled_args: &[Value]) -> Result<Value, GqlError> {
        if evaled_args.len() != 1 {
            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "relationships() expects 1 argument".to_string(),
                actual: format!("{} arguments", evaled_args.len()),
            }));
        }
        match &evaled_args[0] {
            Value::Null => Ok(Value::Null),
            Value::Path { edges, .. } => {
                Ok(Value::List(edges.iter().map(|e| Value::Edge(*e)).collect()))
            }
            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Path".to_string(),
                actual: other.type_name().to_string(),
            })),
        }
    }

    pub(super) fn eval_func_length(
        &self,
        name: &smol_str::SmolStr,
        evaled_args: &[Value],
        distinct: bool,
        ctx: &FilterRow<'_>,
    ) -> Result<Value, GqlError> {
        match evaled_args.first() {
            Some(Value::Path { edges, .. }) => Ok(Value::Int(edges.len() as i64)),
            _ => {
                let lit_args: Vec<Expr> = evaled_args.iter().map(value_to_expr).collect();
                let rebuilt = Expr::FunctionCall {
                    name: name.clone(),
                    args: lit_args,
                    distinct,
                };
                eval_expr(&rebuilt, ctx)
            }
        }
    }

    pub(super) fn eval_func_startnode(
        &self,
        graph: &str,
        evaled_args: &[Value],
    ) -> Result<Value, GqlError> {
        if evaled_args.len() != 1 {
            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "startNode() expects 1 argument".to_string(),
                actual: format!("{} arguments", evaled_args.len()),
            }));
        }
        match &evaled_args[0] {
            Value::Null => Ok(Value::Null),
            Value::Edge(edge_id) => match self.cached_get_edge(graph, *edge_id)? {
                Some(data) => Ok(Value::Node(data.src)),
                None => Ok(Value::Null),
            },
            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Relationship".to_string(),
                actual: other.type_name().to_string(),
            })),
        }
    }

    pub(super) fn eval_func_endnode(
        &self,
        graph: &str,
        evaled_args: &[Value],
    ) -> Result<Value, GqlError> {
        if evaled_args.len() != 1 {
            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "endNode() expects 1 argument".to_string(),
                actual: format!("{} arguments", evaled_args.len()),
            }));
        }
        match &evaled_args[0] {
            Value::Null => Ok(Value::Null),
            Value::Edge(edge_id) => match self.cached_get_edge(graph, *edge_id)? {
                Some(data) => Ok(Value::Node(data.dst)),
                None => Ok(Value::Null),
            },
            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Relationship".to_string(),
                actual: other.type_name().to_string(),
            })),
        }
    }

    pub(super) fn eval_func_gremlin_both_v(
        &self,
        graph: &str,
        evaled_args: Vec<Value>,
    ) -> Result<Value, GqlError> {
        // Return [startNode, endNode] as a list
        match evaled_args.into_iter().next() {
            Some(Value::Edge(edge_id)) => match self.cached_get_edge(graph, edge_id)? {
                Some(data) => Ok(Value::List(vec![
                    Value::Node(data.src),
                    Value::Node(data.dst),
                ])),
                None => Ok(Value::List(vec![])),
            },
            _ => Ok(Value::List(vec![])),
        }
    }

    pub(super) fn eval_func_gremlin_other_v(
        &self,
        graph: &str,
        evaled_args: Vec<Value>,
    ) -> Result<Value, GqlError> {
        // Return the opposite endpoint of an edge relative to
        // the traversal source vertex. Args: (edge, source_vertex).
        let mut args_it = evaled_args.into_iter();
        let edge_val = args_it.next().unwrap_or(Value::Null);
        let src_val = args_it.next().unwrap_or(Value::Null);
        match edge_val {
            Value::Edge(edge_id) => match self.cached_get_edge(graph, edge_id)? {
                Some(data) => {
                    if let Value::Node(src_nid) = src_val {
                        if src_nid == data.src {
                            Ok(Value::Node(data.dst))
                        } else {
                            Ok(Value::Node(data.src))
                        }
                    } else {
                        // No source info — default to dst
                        Ok(Value::Node(data.dst))
                    }
                }
                None => Ok(Value::Null),
            },
            _ => Ok(Value::Null),
        }
    }
}
