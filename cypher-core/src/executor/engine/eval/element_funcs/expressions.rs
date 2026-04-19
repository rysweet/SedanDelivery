//! Element-accessor expression handlers: `Expr::Property`, `Expr::HasLabel`,
//! `Expr::Subscript`.

use crate::ast::Expr;
use crate::error::{ExecutionError, GqlError};
use crate::executor::filter::{eval_expr, Row as FilterRow};
use crate::types::Value;

use super::super::super::helpers::value_to_expr;
use super::super::super::QueryExecutor;
use super::super::super::Row;

impl<'a> QueryExecutor<'a> {
    pub(in crate::executor::engine::eval) fn eval_property(
        &self,
        graph: &str,
        obj_expr: &Expr,
        key: &smol_str::SmolStr,
        row: &Row,
        ctx: &FilterRow<'_>,
    ) -> Result<Value, GqlError> {
        let obj_val = self.eval_with_storage(graph, obj_expr, row, ctx)?;
        if self.gremlin_mode {
            return self.resolve_prop_on_value_for_filter(graph, key.as_str(), &obj_val);
        }
        match obj_val {
            Value::Node(node_id) => match self.cached_get_node(graph, node_id)? {
                Some(node_data) => Ok(node_data
                    .properties
                    .get(key.as_str())
                    .cloned()
                    .unwrap_or(Value::Null)),
                None => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "accessible Node".to_string(),
                    actual: "deleted or missing Node".to_string(),
                })),
            },
            Value::Edge(edge_id) => match self.cached_get_edge(graph, edge_id)? {
                Some(edge_data) => Ok(edge_data
                    .properties
                    .get(key.as_str())
                    .cloned()
                    .unwrap_or(Value::Null)),
                None => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "accessible Relationship".to_string(),
                    actual: "deleted or missing Relationship".to_string(),
                })),
            },
            Value::Map(m) => Ok(m.get(key.as_str()).cloned().unwrap_or(Value::Null)),
            Value::Null => Ok(Value::Null),
            Value::Date(_)
            | Value::LocalTime(_)
            | Value::Time(_, _)
            | Value::LocalDateTime(_)
            | Value::DateTime(_)
            | Value::Duration(_) => {
                crate::executor::filter::temporal_property_access(&obj_val, key.as_str())
            }
            Value::Point(_) => {
                crate::executor::filter::temporal_property_access(&obj_val, key.as_str())
            }
            _ => {
                if self.gremlin_mode {
                    Ok(Value::Null)
                } else {
                    Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "Node, Relationship, or Map".to_string(),
                        actual: obj_val.type_name().to_string(),
                    }))
                }
            }
        }
    }

    pub(in crate::executor::engine::eval) fn eval_has_label(
        &self,
        graph: &str,
        inner: &Expr,
        label: &crate::ast::Label,
        row: &Row,
        ctx: &FilterRow<'_>,
    ) -> Result<Value, GqlError> {
        let mut base_expr: &Expr = inner;
        let mut labels_to_check: Vec<&crate::ast::Label> = vec![label];
        while let Expr::HasLabel(inner_inner, inner_label) = base_expr {
            labels_to_check.push(inner_label);
            base_expr = inner_inner.as_ref();
        }

        let base_val = self.eval_with_storage(graph, base_expr, row, ctx)?;
        match base_val {
            Value::Null => Ok(Value::Null),
            Value::Node(node_id) => match self.cached_get_node(graph, node_id)? {
                Some(node_data) => {
                    let has = labels_to_check
                        .iter()
                        .all(|l| node_data.labels.iter().any(|nl| nl == l.as_str()));
                    Ok(Value::Bool(has))
                }
                None => Ok(Value::Bool(false)),
            },
            Value::Edge(edge_id) => match self.cached_get_edge(graph, edge_id)? {
                Some(edge_data) => {
                    // An edge has exactly one type, so ALL labels must
                    // equal that type; multiple different labels always
                    // returns false.
                    let has = labels_to_check
                        .iter()
                        .all(|l| edge_data.rel_type.as_str() == l.as_str());
                    Ok(Value::Bool(has))
                }
                None => Ok(Value::Bool(false)),
            },
            // Per Cypher semantics: a non-node, non-edge value (e.g. a
            // wildcard pattern result or a scalar) does not have labels, so
            // the label check evaluates to false rather than raising an error.
            _ => Ok(Value::Bool(false)),
        }
    }

    pub(in crate::executor::engine::eval) fn eval_subscript(
        &self,
        graph: &str,
        obj_expr: &Expr,
        idx_expr: &Expr,
        row: &Row,
        ctx: &FilterRow<'_>,
    ) -> Result<Value, GqlError> {
        let obj_val = self.eval_with_storage(graph, obj_expr, row, ctx)?;
        let idx_val = self.eval_with_storage(graph, idx_expr, row, ctx)?;
        match (obj_val, idx_val) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Node(node_id), Value::Str(key)) => {
                match self.cached_get_node(graph, node_id)? {
                    Some(node_data) => Ok(node_data
                        .properties
                        .get(key.as_ref())
                        .cloned()
                        .unwrap_or(Value::Null)),
                    None => Ok(Value::Null),
                }
            }
            (Value::Edge(edge_id), Value::Str(key)) => {
                match self.cached_get_edge(graph, edge_id)? {
                    Some(edge_data) => Ok(edge_data
                        .properties
                        .get(key.as_ref())
                        .cloned()
                        .unwrap_or(Value::Null)),
                    None => Ok(Value::Null),
                }
            }
            (Value::List(list), Value::Int(i)) => {
                let len = list.len() as i64;
                let real_idx = if i < 0 { len + i } else { i };
                if real_idx < 0 || real_idx as usize >= list.len() {
                    Ok(Value::Null)
                } else {
                    Ok(list[real_idx as usize].clone())
                }
            }
            (Value::Map(m), Value::Str(key)) => {
                Ok(m.get(key.as_ref()).cloned().unwrap_or(Value::Null))
            }
            (obj, idx) => {
                let rebuilt =
                    Expr::Subscript(Box::new(value_to_expr(&obj)), Box::new(value_to_expr(&idx)));
                eval_expr(&rebuilt, ctx)
            }
        }
    }
}
