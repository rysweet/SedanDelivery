//! Expression evaluation operators: eval_with_storage, build_filter_ctx,
//! eval_exists_patterns, eval_full_exists_subquery, eval_pattern_comprehension.

use crate::ast::Expr;
use crate::error::{ExecutionError, GqlError};
use crate::executor::budget::ExecutionBudget;
use crate::executor::filter::Row as FilterRow;
use crate::executor::filter::{eval_expr, eval_function_with_vals, quantifier_result};
use crate::planner::QueryPlanner;
use crate::types::Value;

use super::helpers::{eval_binary_values, value_to_expr};
use super::{ExecContext, MutationStats, QueryExecutor, Row};

fn format_value(v: &Value) -> String {
    match v {
        Value::Str(s) => s.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        other => format!("{:?}", other),
    }
}

impl<'a> QueryExecutor<'a> {
    /// Build a `FilterRow` (borrowed string-key map) from an owned `Row`.
    ///
    /// Pre-allocates the `HashMap` with the exact row size to avoid rehashing.
    pub(crate) fn build_filter_ctx<'r>(
        &self,
        _graph: &str,
        row: &'r Row,
    ) -> Result<FilterRow<'r>, GqlError> {
        let mut ctx = FilterRow::with_capacity(row.len());
        for (k, v) in row {
            ctx.insert(k.as_str(), v);
        }
        Ok(ctx)
    }

    /// Evaluate an expression, resolving node/edge property access via the store.
    pub(crate) fn eval_with_storage(
        &self,
        graph: &str,
        expr: &Expr,
        row: &Row,
        ctx: &FilterRow<'_>,
    ) -> Result<Value, GqlError> {
        match expr {
            Expr::Property(obj_expr, key) => {
                let obj_val = self.eval_with_storage(graph, obj_expr, row, ctx)?;
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
                    other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "Node, Relationship, or Map".to_string(),
                        actual: other.type_name().to_string(),
                    })),
                }
            }
            Expr::Variable(id) => {
                if let Some(v) = row.get(id.0.as_str()) {
                    Ok(v.clone())
                } else {
                    eval_expr(expr, ctx)
                }
            }
            Expr::BinaryOp { op, left, right } => {
                let lv = self.eval_with_storage(graph, left, row, ctx)?;
                let rv = self.eval_with_storage(graph, right, row, ctx)?;
                eval_binary_values(*op, lv, rv, ctx)
            }
            Expr::UnaryOp { op, expr: inner } => {
                let v = self.eval_with_storage(graph, inner, row, ctx)?;
                let inner_lit = value_to_expr(&v);
                let rebuilt = Expr::UnaryOp {
                    op: *op,
                    expr: Box::new(inner_lit),
                };
                eval_expr(&rebuilt, ctx)
            }
            Expr::IsNull(inner) => {
                let v = self.eval_with_storage(graph, inner, row, ctx)?;
                Ok(Value::Bool(v.is_null()))
            }
            Expr::IsNotNull(inner) => {
                let v = self.eval_with_storage(graph, inner, row, ctx)?;
                Ok(Value::Bool(!v.is_null()))
            }
            Expr::HasLabel(inner, label) => {
                let mut base_expr: &Expr = inner.as_ref();
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
            Expr::FunctionCall {
                name,
                args,
                distinct,
            } => {
                match name.to_lowercase().as_str() {
                    "all" | "any" | "none" | "single" => {
                        if args.len() == 1 {
                            if let Expr::ListComp {
                                variable,
                                list,
                                filter,
                                ..
                            } = &args[0]
                            {
                                let list_val = self.eval_with_storage(graph, list, row, ctx)?;
                                let items = match list_val {
                                    Value::Null => return Ok(Value::Null),
                                    Value::List(items) => items,
                                    other => {
                                        return Err(GqlError::Execution(
                                            ExecutionError::TypeMismatch {
                                                expected: "List".to_string(),
                                                actual: other.type_name().to_string(),
                                            },
                                        ))
                                    }
                                };
                                let lc = name.to_lowercase();
                                let mut true_count: usize = 0;
                                let mut false_count: usize = 0;
                                let mut null_count: usize = 0;
                                for item in &items {
                                    let mut ext_row = row.clone();
                                    ext_row.insert(variable.0.clone(), item.clone());
                                    let ext_ctx = self.build_filter_ctx(graph, &ext_row)?;
                                    let pred_result = if let Some(pred) = filter {
                                        self.eval_with_storage(graph, pred, &ext_row, &ext_ctx)?
                                    } else {
                                        item.clone()
                                    };
                                    match pred_result {
                                        Value::Bool(true) => true_count += 1,
                                        Value::Bool(false) => false_count += 1,
                                        v if v.is_null() => null_count += 1,
                                        _ => false_count += 1,
                                    }
                                    match lc.as_str() {
                                        "all" if false_count > 0 => return Ok(Value::Bool(false)),
                                        "any" if true_count > 0 => return Ok(Value::Bool(true)),
                                        "none" if true_count > 0 => return Ok(Value::Bool(false)),
                                        "single" if true_count > 1 => {
                                            return Ok(Value::Bool(false))
                                        }
                                        _ => {}
                                    }
                                }
                                return Ok(quantifier_result(
                                    lc.as_str(),
                                    true_count,
                                    false_count,
                                    null_count,
                                ));
                            }
                        }
                        return eval_expr(expr, ctx);
                    }
                    "exists" => {
                        if args.len() == 1 {
                            let val = self.eval_with_storage(graph, &args[0], row, ctx)?;
                            return Ok(Value::Bool(!val.is_null()));
                        }
                        return eval_expr(expr, ctx);
                    }
                    "__patterncomp" => {
                        if args.len() >= 2 {
                            if let Expr::PatternExpr(patterns) = &args[0] {
                                let map_expr = &args[1];
                                let where_expr = args.get(2);
                                return self.eval_pattern_comprehension(
                                    graph, patterns, map_expr, where_expr, row,
                                );
                            }
                        }
                        debug_assert!(false, "__patterncomp: malformed arguments");
                        return Ok(Value::List(vec![]));
                    }
                    _ => {}
                }

                let evaled_args: Result<Vec<Value>, GqlError> = args
                    .iter()
                    .map(|a| self.eval_with_storage(graph, a, row, ctx))
                    .collect();
                let evaled_args = evaled_args?;

                match name.to_lowercase().as_str() {
                    "haslabel" => {
                        if evaled_args.len() < 2 {
                            return Ok(Value::Bool(false));
                        }
                        let label_name = match &evaled_args[1] {
                            Value::Str(s) => s.as_ref().to_string(),
                            _ => return Ok(Value::Bool(false)),
                        };
                        match &evaled_args[0] {
                            Value::Node(node_id) => match self.cached_get_node(graph, *node_id)? {
                                Some(data) => {
                                    Ok(Value::Bool(data.labels.iter().any(|l| l == &label_name)))
                                }
                                None => Ok(Value::Bool(false)),
                            },
                            _ => Ok(Value::Bool(false)),
                        }
                    }
                    "propertiesmatch" => {
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
                    "labels" => {
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
                            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "Node".to_string(),
                                actual: other.type_name().to_string(),
                            })),
                        }
                    }
                    "type" => {
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
                    "id" => {
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
                            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "Node or Relationship".to_string(),
                                actual: other.type_name().to_string(),
                            })),
                        }
                    }
                    "properties" => {
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
                    "keys" => {
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
                            Value::Map(m) => Ok(Value::List(
                                m.keys().map(|k| Value::Str(k.clone())).collect(),
                            )),
                            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "Node, Relationship, or Map".to_string(),
                                actual: other.type_name().to_string(),
                            })),
                        }
                    }
                    "nodes" => {
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
                    "relationships" => {
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
                    "length" => match evaled_args.first() {
                        Some(Value::Path { edges, .. }) => Ok(Value::Int(edges.len() as i64)),
                        _ => {
                            let lit_args: Vec<Expr> =
                                evaled_args.iter().map(value_to_expr).collect();
                            let rebuilt = Expr::FunctionCall {
                                name: name.clone(),
                                args: lit_args,
                                distinct: *distinct,
                            };
                            eval_expr(&rebuilt, ctx)
                        }
                    },
                    "startnode" => {
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
                    "endnode" => {
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
                    "gremlin_valuemap" => {
                        // valueMap(): extract properties and wrap each in a list.
                        // Args: element, with_tokens_bool [, key1, key2, ...]
                        let mut args_iter = evaled_args.into_iter();
                        let element = args_iter.next();
                        let with_tokens = matches!(args_iter.next(), Some(Value::Bool(true)));
                        let keys: Vec<String> = args_iter
                            .filter_map(|v| match v {
                                Value::Str(s) => Some(s.to_string()),
                                _ => None,
                            })
                            .collect();
                        match element {
                            Some(Value::Node(node_id)) => {
                                match self.cached_get_node(graph, node_id)? {
                                    Some(data) => {
                                        let mut wrapped: indexmap::IndexMap<Box<str>, Value> =
                                            indexmap::IndexMap::new();
                                        if with_tokens {
                                            wrapped.insert(
                                                "t[id]".into(),
                                                Value::Int(node_id.0 as i64),
                                            );
                                            let label_str = data
                                                .labels
                                                .first()
                                                .map(|l| l.to_string())
                                                .unwrap_or_default();
                                            wrapped.insert(
                                                "t[label]".into(),
                                                Value::Str(label_str.into()),
                                            );
                                        }
                                        for (k, v) in data.properties.into_iter() {
                                            if keys.is_empty() || keys.iter().any(|fk| fk == &k) {
                                                wrapped.insert(
                                                    k.into_boxed_str(),
                                                    Value::List(vec![v]),
                                                );
                                            }
                                        }
                                        Ok(Value::Map(wrapped))
                                    }
                                    None => Ok(Value::Map(indexmap::IndexMap::new())),
                                }
                            }
                            Some(Value::Edge(edge_id)) => {
                                match self.cached_get_edge(graph, edge_id)? {
                                    Some(data) => {
                                        let mut wrapped: indexmap::IndexMap<Box<str>, Value> =
                                            indexmap::IndexMap::new();
                                        if with_tokens {
                                            wrapped.insert(
                                                "t[id]".into(),
                                                Value::Int(edge_id.0 as i64),
                                            );
                                            wrapped.insert(
                                                "t[label]".into(),
                                                Value::Str(data.rel_type.as_str().into()),
                                            );
                                        }
                                        for (k, v) in data.properties.into_iter() {
                                            if keys.is_empty() || keys.iter().any(|fk| fk == &k) {
                                                wrapped.insert(
                                                    k.into_boxed_str(),
                                                    Value::List(vec![v]),
                                                );
                                            }
                                        }
                                        Ok(Value::Map(wrapped))
                                    }
                                    None => Ok(Value::Map(indexmap::IndexMap::new())),
                                }
                            }
                            Some(Value::Map(m)) => {
                                let wrapped: indexmap::IndexMap<Box<str>, Value> = m
                                    .into_iter()
                                    .filter(|(k, _)| {
                                        keys.is_empty() || keys.iter().any(|fk| fk.as_str() == &**k)
                                    })
                                    .map(|(k, v)| (k, Value::List(vec![v])))
                                    .collect();
                                Ok(Value::Map(wrapped))
                            }
                            Some(other) => Ok(other),
                            None => Ok(Value::Null),
                        }
                    }
                    "gremlin_values_all" => {
                        // values() with no keys: return list of all property values
                        match evaled_args.into_iter().next() {
                            Some(Value::Node(node_id)) => {
                                match self.cached_get_node(graph, node_id)? {
                                    Some(data) => {
                                        let vals: Vec<Value> =
                                            data.properties.into_values().collect();
                                        Ok(Value::List(vals))
                                    }
                                    None => Ok(Value::List(vec![])),
                                }
                            }
                            Some(Value::Edge(edge_id)) => {
                                match self.cached_get_edge(graph, edge_id)? {
                                    Some(data) => {
                                        let vals: Vec<Value> =
                                            data.properties.into_values().collect();
                                        Ok(Value::List(vals))
                                    }
                                    None => Ok(Value::List(vec![])),
                                }
                            }
                            Some(Value::Map(m)) => {
                                let vals: Vec<Value> = m.into_values().collect();
                                Ok(Value::List(vals))
                            }
                            _ => Ok(Value::List(vec![])),
                        }
                    }
                    "gremlin_format" => {
                        // format(): resolve %{key} from node/edge properties.
                        // Returns Null if any %{key} placeholder can't be resolved.
                        match (evaled_args.first(), evaled_args.get(1)) {
                            (Some(Value::Node(node_id)), Some(Value::Str(pattern))) => {
                                match self.cached_get_node(graph, *node_id)? {
                                    Some(data) => {
                                        let mut result = pattern.to_string();
                                        for (k, v) in data.properties.iter() {
                                            let placeholder = format!("%{{{}}}", k);
                                            let val_str = format_value(v);
                                            result = result.replace(&placeholder, &val_str);
                                        }
                                        if result.contains("%{") {
                                            Ok(Value::Null)
                                        } else {
                                            Ok(Value::Str(result.into()))
                                        }
                                    }
                                    None => Ok(Value::Null),
                                }
                            }
                            (Some(Value::Edge(edge_id)), Some(Value::Str(pattern))) => {
                                match self.cached_get_edge(graph, *edge_id)? {
                                    Some(data) => {
                                        let mut result = pattern.to_string();
                                        for (k, v) in data.properties.iter() {
                                            let placeholder = format!("%{{{}}}", k);
                                            let val_str = format_value(v);
                                            result = result.replace(&placeholder, &val_str);
                                        }
                                        if result.contains("%{") {
                                            Ok(Value::Null)
                                        } else {
                                            Ok(Value::Str(result.into()))
                                        }
                                    }
                                    None => Ok(Value::Null),
                                }
                            }
                            (Some(Value::Str(_)), Some(Value::Str(pattern))) => {
                                // Format with a string value (no properties to resolve)
                                if !pattern.contains("%{") {
                                    Ok(Value::Str(pattern.clone()))
                                } else {
                                    eval_function_with_vals(name, evaled_args)
                                }
                            }
                            _ => eval_function_with_vals(name, evaled_args),
                        }
                    }
                    _ => eval_function_with_vals(name, evaled_args),
                }
            }
            Expr::Case {
                subject,
                alternatives,
                default,
            } => {
                if let Some(subj) = subject {
                    let subj_val = self.eval_with_storage(graph, subj, row, ctx)?;
                    for (when_expr, then_expr) in alternatives {
                        let when_val = self.eval_with_storage(graph, when_expr, row, ctx)?;
                        if subj_val == when_val {
                            return self.eval_with_storage(graph, then_expr, row, ctx);
                        }
                    }
                } else {
                    for (when_expr, then_expr) in alternatives {
                        let when_val = self.eval_with_storage(graph, when_expr, row, ctx)?;
                        if when_val == Value::Bool(true) {
                            return self.eval_with_storage(graph, then_expr, row, ctx);
                        }
                    }
                }
                match default {
                    Some(d) => self.eval_with_storage(graph, d, row, ctx),
                    None => Ok(Value::Null),
                }
            }
            Expr::List(items) => {
                let mut result = Vec::with_capacity(items.len());
                for item in items {
                    result.push(self.eval_with_storage(graph, item, row, ctx)?);
                }
                Ok(Value::List(result))
            }
            Expr::MapLiteral(pairs) => {
                let mut map = indexmap::IndexMap::new();
                for (key, val_expr) in pairs {
                    let val = self.eval_with_storage(graph, val_expr, row, ctx)?;
                    map.insert(key.to_string().into_boxed_str(), val);
                }
                Ok(Value::Map(map))
            }
            Expr::ListComp {
                variable,
                list,
                filter,
                map,
            } => {
                let list_val = self.eval_with_storage(graph, list, row, ctx)?;
                match list_val {
                    Value::Null => Ok(Value::Null),
                    Value::List(items) => {
                        let mut result = Vec::with_capacity(items.len());
                        for item in &items {
                            let mut extended_row = row.clone();
                            extended_row.insert(variable.0.clone(), item.clone());
                            let ext_ctx = self.build_filter_ctx(graph, &extended_row)?;
                            if let Some(pred) = filter {
                                match self.eval_with_storage(
                                    graph,
                                    pred,
                                    &extended_row,
                                    &ext_ctx,
                                )? {
                                    Value::Bool(true) => {}
                                    _ => continue,
                                }
                            }
                            result.push(self.eval_with_storage(
                                graph,
                                map,
                                &extended_row,
                                &ext_ctx,
                            )?);
                        }
                        Ok(Value::List(result))
                    }
                    other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "List".to_string(),
                        actual: other.type_name().to_string(),
                    })),
                }
            }
            Expr::Reduce {
                accumulator,
                initial,
                variable,
                list,
                expression,
            } => {
                let init_val = self.eval_with_storage(graph, initial, row, ctx)?;
                let list_val = self.eval_with_storage(graph, list, row, ctx)?;
                match list_val {
                    Value::Null => Ok(Value::Null),
                    Value::List(items) => {
                        let mut acc = init_val;
                        for item in &items {
                            let mut extended_row = row.clone();
                            extended_row.insert(accumulator.0.clone(), acc);
                            extended_row.insert(variable.0.clone(), item.clone());
                            let ext_ctx = self.build_filter_ctx(graph, &extended_row)?;
                            acc =
                                self.eval_with_storage(graph, expression, &extended_row, &ext_ctx)?;
                        }
                        Ok(acc)
                    }
                    other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "List".to_string(),
                        actual: other.type_name().to_string(),
                    })),
                }
            }
            Expr::ExistsSubquery {
                patterns,
                where_clause,
                full_query,
            } => {
                if let Some(inner_query) = full_query {
                    self.eval_full_exists_subquery(graph, inner_query, row)
                } else {
                    self.eval_exists_patterns(graph, patterns, where_clause.as_deref(), row)
                }
            }
            Expr::PatternExpr(patterns) => self.eval_exists_patterns(graph, patterns, None, row),
            Expr::Subscript(obj_expr, idx_expr) => {
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
                        let rebuilt = Expr::Subscript(
                            Box::new(value_to_expr(&obj)),
                            Box::new(value_to_expr(&idx)),
                        );
                        eval_expr(&rebuilt, ctx)
                    }
                }
            }
            _ => eval_expr(expr, ctx),
        }
    }

    pub(crate) fn eval_exists_patterns(
        &self,
        graph: &str,
        patterns: &[crate::ast::PathPattern],
        where_clause: Option<&Expr>,
        outer_row: &Row,
    ) -> Result<Value, GqlError> {
        let planner = QueryPlanner::new(self.store, graph);
        let sub_op = planner.plan_exists_subquery(patterns, where_clause)?;

        let outer = vec![outer_row.clone()];
        let mut budget = ExecutionBudget::default_budget();
        let mut stats = MutationStats::default();
        let mut sub_ctx = ExecContext {
            graph,
            budget: &mut budget,
            outer_rows: &outer,
            stats: &mut stats,
        };
        let sub_rows = self.exec_operator(&mut sub_ctx, &sub_op)?;

        let has_match = sub_rows.iter().any(|sub_row| {
            outer_row
                .iter()
                .filter(|(k, _)| !k.starts_with("__anon"))
                .all(|(k, v)| sub_row.get(k).is_none_or(|sv| sv == v))
        });
        Ok(Value::Bool(has_match))
    }

    pub(crate) fn eval_full_exists_subquery(
        &self,
        graph: &str,
        inner_query: &crate::ast::Query,
        outer_row: &Row,
    ) -> Result<Value, GqlError> {
        let planner =
            QueryPlanner::with_context(self.store, graph, self.registry, &self.parameters);
        let inner_op = planner.plan_unchecked(inner_query)?;
        let outer = vec![outer_row.clone()];
        let mut budget = ExecutionBudget::default_budget();
        let mut stats = MutationStats::default();
        let mut sub_ctx = ExecContext {
            graph,
            budget: &mut budget,
            outer_rows: &outer,
            stats: &mut stats,
        };
        let sub_rows = self.exec_operator(&mut sub_ctx, &inner_op)?;
        let has_match = sub_rows.iter().any(|sub_row| {
            outer_row
                .iter()
                .filter(|(k, _)| !k.starts_with('$') && !k.starts_with("__anon"))
                .all(|(k, v)| sub_row.get(k).is_none_or(|sv| sv == v))
        });
        Ok(Value::Bool(has_match))
    }

    pub(crate) fn eval_pattern_comprehension(
        &self,
        graph: &str,
        patterns: &[crate::ast::PathPattern],
        map_expr: &Expr,
        where_expr: Option<&Expr>,
        outer_row: &Row,
    ) -> Result<Value, GqlError> {
        let planner =
            QueryPlanner::with_context(self.store, graph, self.registry, &self.parameters);
        let sub_op = planner.plan_exists_subquery(patterns, None)?;

        let outer = vec![outer_row.clone()];
        let mut budget = ExecutionBudget::default_budget();
        let mut stats = MutationStats::default();
        let mut sub_ctx = ExecContext {
            graph,
            budget: &mut budget,
            outer_rows: &outer,
            stats: &mut stats,
        };
        let sub_rows = self.exec_operator(&mut sub_ctx, &sub_op)?;

        let mut result = Vec::new();
        for sub_row in &sub_rows {
            let compatible = outer_row
                .iter()
                .filter(|(k, _)| !k.starts_with('$') && !k.starts_with("__anon"))
                .all(|(k, v)| sub_row.get(k.as_str()).is_none_or(|sv| sv == v));
            if !compatible {
                continue;
            }
            if let Some(pred) = where_expr {
                if !matches!(pred, Expr::Bool(true)) {
                    let ctx = self.build_filter_ctx(graph, sub_row)?;
                    match self.eval_with_storage(graph, pred, sub_row, &ctx)? {
                        Value::Bool(true) => {}
                        _ => continue,
                    }
                }
            }
            let ctx = self.build_filter_ctx(graph, sub_row)?;
            result.push(self.eval_with_storage(graph, map_expr, sub_row, &ctx)?);
        }
        Ok(Value::List(result))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOp, Identifier};
    use crate::storage::{GraphStore, InMemoryGraphStore};
    use std::collections::HashMap;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    #[test]
    fn eval_property_on_node() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let nid = store
            .create_node("g", vec!["Person".to_string()], props)
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("n".to_string(), Value::Node(nid));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::Property(
            Box::new(Expr::Variable(Identifier::new("n"))),
            "name".into(),
        );
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Str("Alice".into()));
    }

    #[test]
    fn eval_property_on_edge() {
        let store = make_store();
        let a = store.create_node("g", vec![], HashMap::new()).unwrap();
        let b = store.create_node("g", vec![], HashMap::new()).unwrap();
        let mut props = HashMap::new();
        props.insert("weight".to_string(), Value::Int(42));
        let eid = store
            .create_edge("g", a, b, "R".to_string(), props)
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("r".to_string(), Value::Edge(eid));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::Property(
            Box::new(Expr::Variable(Identifier::new("r"))),
            "weight".into(),
        );
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn eval_property_on_null() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("n".to_string(), Value::Null);

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::Property(
            Box::new(Expr::Variable(Identifier::new("n"))),
            "name".into(),
        );
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn eval_property_on_map() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut map = indexmap::IndexMap::new();
        map.insert("key".to_string().into_boxed_str(), Value::Int(99));
        let mut row = Row::new();
        row.insert("m".to_string(), Value::Map(map));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::Property(Box::new(Expr::Variable(Identifier::new("m"))), "key".into());
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Int(99));
    }

    #[test]
    fn eval_is_null_and_is_not_null() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("x".to_string(), Value::Null);
        row.insert("y".to_string(), Value::Int(1));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let is_null_expr = Expr::IsNull(Box::new(Expr::Variable(Identifier::new("x"))));
        assert_eq!(
            executor
                .eval_with_storage("g", &is_null_expr, &row, &ctx)
                .unwrap(),
            Value::Bool(true)
        );

        let is_not_null_expr = Expr::IsNotNull(Box::new(Expr::Variable(Identifier::new("y"))));
        assert_eq!(
            executor
                .eval_with_storage("g", &is_not_null_expr, &row, &ctx)
                .unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn eval_binary_op_addition() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("a".to_string(), Value::Int(10));
        row.insert("b".to_string(), Value::Int(20));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::BinaryOp {
            op: BinaryOp::Add,
            left: Box::new(Expr::Variable(Identifier::new("a"))),
            right: Box::new(Expr::Variable(Identifier::new("b"))),
        };
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Int(30));
    }

    #[test]
    fn eval_has_label_on_node() {
        let store = make_store();
        let nid = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("n".to_string(), Value::Node(nid));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::HasLabel(
            Box::new(Expr::Variable(Identifier::new("n"))),
            crate::ast::Label::new("Person"),
        );
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Bool(true));

        let expr_wrong = Expr::HasLabel(
            Box::new(Expr::Variable(Identifier::new("n"))),
            crate::ast::Label::new("Movie"),
        );
        let result2 = executor
            .eval_with_storage("g", &expr_wrong, &row, &ctx)
            .unwrap();
        assert_eq!(result2, Value::Bool(false));
    }

    #[test]
    fn eval_function_id() {
        let store = make_store();
        let nid = store.create_node("g", vec![], HashMap::new()).unwrap();

        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("n".to_string(), Value::Node(nid));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::FunctionCall {
            name: "id".into(),
            args: vec![Expr::Variable(Identifier::new("n"))],
            distinct: false,
        };
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Int(nid.0 as i64));
    }

    #[test]
    fn eval_function_labels() {
        let store = make_store();
        let nid = store
            .create_node("g", vec!["A".to_string(), "B".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("n".to_string(), Value::Node(nid));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::FunctionCall {
            name: "labels".into(),
            args: vec![Expr::Variable(Identifier::new("n"))],
            distinct: false,
        };
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        match result {
            Value::List(items) => {
                assert_eq!(items.len(), 2);
            }
            other => panic!("expected List, got {:?}", other),
        }
    }

    #[test]
    fn eval_function_type() {
        let store = make_store();
        let a = store.create_node("g", vec![], HashMap::new()).unwrap();
        let b = store.create_node("g", vec![], HashMap::new()).unwrap();
        let eid = store
            .create_edge("g", a, b, "KNOWS".to_string(), HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("r".to_string(), Value::Edge(eid));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::FunctionCall {
            name: "type".into(),
            args: vec![Expr::Variable(Identifier::new("r"))],
            distinct: false,
        };
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Str("KNOWS".into()));
    }

    #[test]
    fn eval_function_properties() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let nid = store
            .create_node("g", vec!["Person".to_string()], props)
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("n".to_string(), Value::Node(nid));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::FunctionCall {
            name: "properties".into(),
            args: vec![Expr::Variable(Identifier::new("n"))],
            distinct: false,
        };
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        match result {
            Value::Map(m) => {
                assert_eq!(m.get("name"), Some(&Value::Str("Alice".into())));
            }
            other => panic!("expected Map, got {:?}", other),
        }
    }

    #[test]
    fn eval_function_keys() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("a".to_string(), Value::Int(1));
        props.insert("b".to_string(), Value::Int(2));
        let nid = store.create_node("g", vec![], props).unwrap();

        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("n".to_string(), Value::Node(nid));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::FunctionCall {
            name: "keys".into(),
            args: vec![Expr::Variable(Identifier::new("n"))],
            distinct: false,
        };
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        match result {
            Value::List(items) => {
                assert_eq!(items.len(), 2);
            }
            other => panic!("expected List, got {:?}", other),
        }
    }

    #[test]
    fn eval_case_expression() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("x".to_string(), Value::Int(1));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::Case {
            subject: Some(Box::new(Expr::Variable(Identifier::new("x")))),
            alternatives: vec![
                (Expr::Integer(1), Expr::Str("one".into())),
                (Expr::Integer(2), Expr::Str("two".into())),
            ],
            default: Some(Box::new(Expr::Str("other".into()))),
        };
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Str("one".into()));
    }

    #[test]
    fn eval_case_no_subject() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("x".to_string(), Value::Int(5));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::Case {
            subject: None,
            alternatives: vec![(
                Expr::BinaryOp {
                    op: BinaryOp::Gt,
                    left: Box::new(Expr::Variable(Identifier::new("x"))),
                    right: Box::new(Expr::Integer(3)),
                },
                Expr::Str("big".into()),
            )],
            default: Some(Box::new(Expr::Str("small".into()))),
        };
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Str("big".into()));
    }

    #[test]
    fn eval_case_default_fallback() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("x".to_string(), Value::Int(99));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        // No matching alternative
        let expr = Expr::Case {
            subject: Some(Box::new(Expr::Variable(Identifier::new("x")))),
            alternatives: vec![(Expr::Integer(1), Expr::Str("one".into()))],
            default: None,
        };
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn eval_list_literal() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let row = Row::new();
        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::List(vec![Expr::Integer(1), Expr::Integer(2), Expr::Integer(3)]);
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(
            result,
            Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
        );
    }

    #[test]
    fn eval_map_literal() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let row = Row::new();
        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::MapLiteral(vec![(smol_str::SmolStr::new("a"), Expr::Integer(1))]);
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        match result {
            Value::Map(m) => {
                assert_eq!(m.get("a"), Some(&Value::Int(1)));
            }
            other => panic!("expected Map, got {:?}", other),
        }
    }

    #[test]
    fn eval_subscript_list() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert(
            "xs".to_string(),
            Value::List(vec![Value::Int(10), Value::Int(20), Value::Int(30)]),
        );

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::Subscript(
            Box::new(Expr::Variable(Identifier::new("xs"))),
            Box::new(Expr::Integer(1)),
        );
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Int(20));
    }

    #[test]
    fn eval_subscript_negative_index() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert(
            "xs".to_string(),
            Value::List(vec![Value::Int(10), Value::Int(20), Value::Int(30)]),
        );

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::Subscript(
            Box::new(Expr::Variable(Identifier::new("xs"))),
            Box::new(Expr::Integer(-1)),
        );
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Int(30));
    }

    #[test]
    fn eval_subscript_out_of_bounds() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("xs".to_string(), Value::List(vec![Value::Int(10)]));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::Subscript(
            Box::new(Expr::Variable(Identifier::new("xs"))),
            Box::new(Expr::Integer(5)),
        );
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn eval_startnode_endnode() {
        let store = make_store();
        let a = store.create_node("g", vec![], HashMap::new()).unwrap();
        let b = store.create_node("g", vec![], HashMap::new()).unwrap();
        let eid = store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("r".to_string(), Value::Edge(eid));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();

        let start_expr = Expr::FunctionCall {
            name: "startNode".into(),
            args: vec![Expr::Variable(Identifier::new("r"))],
            distinct: false,
        };
        let result = executor
            .eval_with_storage("g", &start_expr, &row, &ctx)
            .unwrap();
        assert_eq!(result, Value::Node(a));

        let end_expr = Expr::FunctionCall {
            name: "endNode".into(),
            args: vec![Expr::Variable(Identifier::new("r"))],
            distinct: false,
        };
        let result2 = executor
            .eval_with_storage("g", &end_expr, &row, &ctx)
            .unwrap();
        assert_eq!(result2, Value::Node(b));
    }

    #[test]
    fn eval_nodes_and_relationships_on_path() {
        let store = make_store();
        let a = store.create_node("g", vec![], HashMap::new()).unwrap();
        let b = store.create_node("g", vec![], HashMap::new()).unwrap();
        let eid = store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert(
            "p".to_string(),
            Value::Path {
                nodes: vec![a, b],
                edges: vec![eid],
            },
        );

        let ctx = executor.build_filter_ctx("g", &row).unwrap();

        let nodes_expr = Expr::FunctionCall {
            name: "nodes".into(),
            args: vec![Expr::Variable(Identifier::new("p"))],
            distinct: false,
        };
        let result = executor
            .eval_with_storage("g", &nodes_expr, &row, &ctx)
            .unwrap();
        assert_eq!(result, Value::List(vec![Value::Node(a), Value::Node(b)]));

        let rels_expr = Expr::FunctionCall {
            name: "relationships".into(),
            args: vec![Expr::Variable(Identifier::new("p"))],
            distinct: false,
        };
        let result2 = executor
            .eval_with_storage("g", &rels_expr, &row, &ctx)
            .unwrap();
        assert_eq!(result2, Value::List(vec![Value::Edge(eid)]));
    }

    #[test]
    fn eval_length_on_path() {
        let store = make_store();
        let a = store.create_node("g", vec![], HashMap::new()).unwrap();
        let b = store.create_node("g", vec![], HashMap::new()).unwrap();
        let eid = store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert(
            "p".to_string(),
            Value::Path {
                nodes: vec![a, b],
                edges: vec![eid],
            },
        );

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::FunctionCall {
            name: "length".into(),
            args: vec![Expr::Variable(Identifier::new("p"))],
            distinct: false,
        };
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Int(1));
    }

    #[test]
    fn eval_list_comprehension() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert(
            "xs".to_string(),
            Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
        );

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::ListComp {
            variable: Identifier::new("x"),
            list: Box::new(Expr::Variable(Identifier::new("xs"))),
            filter: Some(Box::new(Expr::BinaryOp {
                op: BinaryOp::Gt,
                left: Box::new(Expr::Variable(Identifier::new("x"))),
                right: Box::new(Expr::Integer(1)),
            })),
            map: Box::new(Expr::BinaryOp {
                op: BinaryOp::Mul,
                left: Box::new(Expr::Variable(Identifier::new("x"))),
                right: Box::new(Expr::Integer(10)),
            }),
        };
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::List(vec![Value::Int(20), Value::Int(30)]));
    }

    #[test]
    fn eval_function_id_null_returns_null() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("x".to_string(), Value::Null);

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::FunctionCall {
            name: "id".into(),
            args: vec![Expr::Variable(Identifier::new("x"))],
            distinct: false,
        };
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn eval_function_labels_null_returns_null() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("x".to_string(), Value::Null);

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::FunctionCall {
            name: "labels".into(),
            args: vec![Expr::Variable(Identifier::new("x"))],
            distinct: false,
        };
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn eval_function_type_null_returns_null() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("x".to_string(), Value::Null);

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::FunctionCall {
            name: "type".into(),
            args: vec![Expr::Variable(Identifier::new("x"))],
            distinct: false,
        };
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn eval_function_nodes_null_returns_null() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("x".to_string(), Value::Null);

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::FunctionCall {
            name: "nodes".into(),
            args: vec![Expr::Variable(Identifier::new("x"))],
            distinct: false,
        };
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn eval_function_relationships_null_returns_null() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("x".to_string(), Value::Null);

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::FunctionCall {
            name: "relationships".into(),
            args: vec![Expr::Variable(Identifier::new("x"))],
            distinct: false,
        };
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn eval_function_id_on_edge() {
        let store = make_store();
        let a = store.create_node("g", vec![], HashMap::new()).unwrap();
        let b = store.create_node("g", vec![], HashMap::new()).unwrap();
        let eid = store
            .create_edge("g", a, b, "KNOWS".to_string(), HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("r".to_string(), Value::Edge(eid));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::FunctionCall {
            name: "id".into(),
            args: vec![Expr::Variable(Identifier::new("r"))],
            distinct: false,
        };
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        assert_eq!(result, Value::Int(eid.0 as i64));
    }

    #[test]
    fn eval_function_labels_multi() {
        let store = make_store();
        let nid = store
            .create_node("g", vec!["A".to_string(), "B".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("n".to_string(), Value::Node(nid));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        let expr = Expr::FunctionCall {
            name: "labels".into(),
            args: vec![Expr::Variable(Identifier::new("n"))],
            distinct: false,
        };
        let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
        match result {
            Value::List(items) => {
                let strs: Vec<&str> = items.iter().map(|v| v.as_str().unwrap()).collect();
                assert!(strs.contains(&"A"));
                assert!(strs.contains(&"B"));
                assert_eq!(strs.len(), 2);
            }
            other => panic!("expected List, got {:?}", other),
        }
    }

    #[test]
    fn build_filter_ctx_basic() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let mut row = Row::new();
        row.insert("a".to_string(), Value::Int(1));
        row.insert("b".to_string(), Value::Str("hello".into()));

        let ctx = executor.build_filter_ctx("g", &row).unwrap();
        assert_eq!(ctx.get("a"), Some(&&Value::Int(1)));
        assert_eq!(ctx.get("b"), Some(&&Value::Str("hello".into())));
    }
}
