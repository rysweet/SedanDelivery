//! Element-accessor expression and function handlers split out of `eval`:
//! `Expr::Property`, `Expr::HasLabel`, `Expr::Subscript`, and the function
//! arms `id`, `labels`, `type`, `properties`, `keys`, `propertiesMatch`,
//! `hasLabel`, gremlin `T.label/T.key/T.value`.

use crate::ast::Expr;
use crate::error::{ExecutionError, GqlError};
use crate::executor::filter::{eval_expr, map_key_to_value, Row as FilterRow};
use crate::types::Value;

use super::super::helpers::value_to_expr;
use super::super::QueryExecutor;
use super::super::Row;

impl<'a> QueryExecutor<'a> {
    pub(super) fn eval_property(
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

    pub(super) fn eval_has_label(
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

    pub(super) fn eval_subscript(
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

    pub(super) fn eval_func_has_label(
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

    pub(super) fn eval_func_properties_match(
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

    pub(super) fn eval_func_labels(
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

    // Gremlin T.label: returns single label string (not a list)
    pub(super) fn eval_func_gremlin_single_label(
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
    pub(super) fn eval_func_gremlin_property_key(
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
    pub(super) fn eval_func_gremlin_property_value(
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

    pub(super) fn eval_func_type(
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

    pub(super) fn eval_func_id(&self, evaled_args: &[Value]) -> Result<Value, GqlError> {
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

    pub(super) fn eval_func_properties(
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

    pub(super) fn eval_func_keys(
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
