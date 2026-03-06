//! Utility functions for the query executor.

use std::collections::HashMap;

use crate::ast::{Direction, Expr};
use crate::error::{ExecutionError, GqlError};
use crate::executor::filter::Row as FilterRow;
use crate::executor::filter::{eval_expr, time_to_utc_nanos};
use crate::planner::Operator;
use crate::storage::{GraphStore, NodeId};
use crate::types::Value;

use super::Row;

/// Extract a `NodeId` from a `Value::Node(id)`, returning an error if the
/// value is not a node reference.
pub(crate) fn extract_node_id(val: &Value, var: &str) -> Result<NodeId, GqlError> {
    match val {
        Value::Node(id) => Ok(*id),
        other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: format!("Node for variable '{var}'"),
            actual: other.type_name().to_string(),
        })),
    }
}

/// Build an error for an undefined variable.
pub(crate) fn undefined_var(name: &str) -> GqlError {
    GqlError::Execution(ExecutionError::TypeMismatch {
        expected: format!("variable '{name}' in scope"),
        actual: "undefined".to_string(),
    })
}

pub(crate) fn get_adj_entries(
    store: &dyn GraphStore,
    graph: &str,
    node_id: NodeId,
    direction: &Direction,
    type_strs: &[&str],
) -> Result<Vec<crate::storage::AdjEntry>, GqlError> {
    let entries = match direction {
        Direction::Outgoing => store.out_edges_by_type(graph, node_id, type_strs)?,
        Direction::Incoming => store.in_edges_by_type(graph, node_id, type_strs)?,
        Direction::Undirected => {
            let mut out = store.out_edges_by_type(graph, node_id, type_strs)?;
            let in_ = store.in_edges_by_type(graph, node_id, type_strs)?;
            out.extend(in_);
            out
        }
    };
    Ok(entries)
}

/// Discover the aliases (column names) that an operator tree would produce.
/// Used by LeftOuterApply to fill NULLs for the right side when it produces no rows.
pub(crate) fn discover_operator_aliases(op: &Operator) -> Vec<String> {
    let mut aliases = Vec::new();
    match op {
        Operator::NodeScan { alias, .. } => {
            aliases.push(alias.0.clone());
        }
        Operator::IndexScan { alias, .. } => {
            aliases.push(alias.0.clone());
        }
        Operator::NodeByIdSeek { alias, .. } => {
            aliases.push(alias.0.clone());
        }
        Operator::Expand {
            rel_alias,
            dst_alias,
            child,
            ..
        } => {
            aliases.extend(discover_operator_aliases(child));
            if let Some(ra) = rel_alias {
                aliases.push(ra.0.clone());
            }
            aliases.push(dst_alias.0.clone());
        }
        Operator::VarLenExpand {
            path_alias,
            dst_alias,
            child,
            ..
        } => {
            aliases.extend(discover_operator_aliases(child));
            if let Some(pa) = path_alias {
                aliases.push(pa.0.clone());
            }
            aliases.push(dst_alias.0.clone());
        }
        Operator::Filter { child, .. }
        | Operator::Sort { child, .. }
        | Operator::Limit { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Distinct { child, .. }
        | Operator::Set { child, .. }
        | Operator::Remove { child, .. }
        | Operator::Delete { child, .. }
        | Operator::Unwind { child, .. } => {
            aliases.extend(discover_operator_aliases(child));
        }
        Operator::AssemblePath {
            path_alias, child, ..
        } => {
            aliases.extend(discover_operator_aliases(child));
            aliases.push(path_alias.0.clone());
        }
        Operator::Project { child, .. } => {
            aliases.extend(discover_operator_aliases(child));
        }
        Operator::Aggregate { child, .. } => {
            aliases.extend(discover_operator_aliases(child));
        }
        Operator::MergeRelationship {
            child, rel_alias, ..
        } => {
            aliases.extend(discover_operator_aliases(child));
            if let Some(ra) = rel_alias {
                aliases.push(ra.0.clone());
            }
        }
        _ => {}
    }
    aliases
}

/// Inject query parameters into a row.
///
/// Uses `entry().or_insert_with()` so that values already present in the row
/// (computed by earlier operators) are never overwritten by a parameter of the
/// same name.  On an empty row the behaviour is identical to plain `insert`.
pub(crate) fn inject_parameters(row: &mut Row, params: &HashMap<String, Value>) {
    for (k, v) in params {
        row.entry(k.clone()).or_insert_with(|| v.clone());
    }
}

/// Generate a deterministic string key for a row, used by `Distinct`.
pub(crate) fn row_key(row: &Row) -> String {
    let mut pairs: Vec<(&str, String)> = row
        .iter()
        .map(|(k, v)| (k.as_str(), v.canonical_key()))
        .collect();
    pairs.sort_by_key(|(k, _)| *k);
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("|")
}

/// Evaluate a `count` expression (LIMIT/SKIP argument) — must be a non-negative integer.
pub(crate) fn eval_count_expr(expr: &Expr, params: &Row) -> Result<usize, GqlError> {
    let ctx: FilterRow<'_> = params.iter().map(|(k, v)| (k.as_str(), v)).collect();
    match eval_expr(expr, &ctx)? {
        Value::Int(n) if n >= 0 => Ok(n as usize),
        Value::Int(n) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "non-negative integer for LIMIT/SKIP".to_string(),
            actual: n.to_string(),
        })),
        other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "integer for LIMIT/SKIP".to_string(),
            actual: other.type_name().to_string(),
        })),
    }
}

/// Derive a column name string from an expression.
/// `n.name` -> `"n.name"`, `n` -> `"n"`, literal -> `"literal"`.
/// Returns the precedence level of a binary operator for column-name parenthesization.
/// Higher value = higher precedence (binds tighter).
fn binop_col_prec(op: &crate::ast::BinaryOp) -> i32 {
    use crate::ast::BinaryOp;
    match op {
        BinaryOp::Pow => 6,
        BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => 5,
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Concat => 4,
        _ => 3,
    }
}

/// True for operators where `a op (b op c) != (a op b) op c`,
/// so the right child needs parentheses when it has the same precedence.
fn binop_col_nonassoc(op: &crate::ast::BinaryOp) -> bool {
    use crate::ast::BinaryOp;
    matches!(op, BinaryOp::Sub | BinaryOp::Div | BinaryOp::Mod)
}

pub(crate) fn expr_to_column_name(expr: &Expr) -> String {
    match expr {
        Expr::Variable(id) => id.0.clone(),
        Expr::Property(obj, key) => {
            // Subscript expressions as property objects are wrapped in parens
            // to match the query text: `(list[1]).key` -> column `(list[1]).key`.
            let obj_str = match obj.as_ref() {
                Expr::Subscript(_, _) => format!("({})", expr_to_column_name(obj)),
                _ => expr_to_column_name(obj),
            };
            format!("{}.{}", obj_str, key)
        }
        Expr::Integer(n) => n.to_string(),
        Expr::Float(f) => f.to_string(),
        Expr::Str(s) => format!("'{s}'"),
        Expr::Bool(b) => b.to_string(),
        Expr::Null => "null".to_string(),
        Expr::FunctionCall {
            name,
            args,
            distinct,
        } => {
            let arg_strs: Vec<String> = args.iter().map(expr_to_column_name).collect();
            if *distinct {
                format!("{}(DISTINCT {})", name, arg_strs.join(", "))
            } else {
                format!("{}({})", name, arg_strs.join(", "))
            }
        }
        Expr::UnaryOp {
            op: crate::ast::UnaryOp::Neg,
            expr: inner,
        } => format!("-{}", expr_to_column_name(inner)),
        Expr::UnaryOp {
            op: crate::ast::UnaryOp::Not,
            expr: inner,
        } => format!("NOT {}", expr_to_column_name(inner)),
        Expr::BinaryOp { op, left, right } => {
            let op_str = match op {
                crate::ast::BinaryOp::Add => "+",
                crate::ast::BinaryOp::Sub => "-",
                crate::ast::BinaryOp::Mul => "*",
                crate::ast::BinaryOp::Div => "/",
                crate::ast::BinaryOp::Mod => "%",
                crate::ast::BinaryOp::Eq => "=",
                crate::ast::BinaryOp::Neq => "<>",
                crate::ast::BinaryOp::Lt => "<",
                crate::ast::BinaryOp::Lte => "<=",
                crate::ast::BinaryOp::Gt => ">",
                crate::ast::BinaryOp::Gte => ">=",
                crate::ast::BinaryOp::And => "AND",
                crate::ast::BinaryOp::Or => "OR",
                crate::ast::BinaryOp::Xor => "XOR",
                crate::ast::BinaryOp::In => "IN",
                crate::ast::BinaryOp::Contains => "CONTAINS",
                crate::ast::BinaryOp::StartsWith => "STARTS WITH",
                crate::ast::BinaryOp::EndsWith => "ENDS WITH",
                crate::ast::BinaryOp::Pow => "^",
                crate::ast::BinaryOp::RegexMatch => "=~",
                crate::ast::BinaryOp::Concat => "++",
            };
            let parent_prec = binop_col_prec(op);
            // Left child: wrap if strictly lower precedence
            let left_str = match left.as_ref() {
                Expr::BinaryOp { op: child_op, .. } if binop_col_prec(child_op) < parent_prec => {
                    format!("({})", expr_to_column_name(left))
                }
                _ => expr_to_column_name(left),
            };
            // Right child: wrap if lower precedence, or same prec for non-associative ops
            let right_str = match right.as_ref() {
                Expr::BinaryOp { op: child_op, .. }
                    if binop_col_prec(child_op) < parent_prec
                        || (binop_col_prec(child_op) == parent_prec && binop_col_nonassoc(op)) =>
                {
                    format!("({})", expr_to_column_name(right))
                }
                _ => expr_to_column_name(right),
            };
            format!("{} {} {}", left_str, op_str, right_str)
        }
        Expr::ListComp {
            variable,
            list,
            filter,
            map,
        } => {
            let list_str = expr_to_column_name(list);
            let var_str = &variable.0;
            if let Some(f) = filter {
                let filter_str = expr_to_column_name(f);
                if **map != Expr::Variable(variable.clone()) {
                    format!(
                        "[{} IN {} WHERE {} | {}]",
                        var_str,
                        list_str,
                        filter_str,
                        expr_to_column_name(map)
                    )
                } else {
                    format!("[{} IN {} WHERE {}]", var_str, list_str, filter_str)
                }
            } else {
                format!("[{} IN {}]", var_str, list_str)
            }
        }
        // These must match expr_to_col_name in planner.rs exactly.
        Expr::IsNull(e) => format!("{} IS NULL", expr_to_column_name(e)),
        Expr::IsNotNull(e) => format!("{} IS NOT NULL", expr_to_column_name(e)),
        Expr::HasLabel(e, label) => format!("{}:{}", expr_to_column_name(e), label.0),
        Expr::Parameter(name) => format!("${name}"),
        Expr::List(items) => {
            let items_str: Vec<String> = items.iter().map(expr_to_column_name).collect();
            format!("[{}]", items_str.join(", "))
        }
        Expr::Subscript(list, idx) => {
            format!(
                "{}[{}]",
                expr_to_column_name(list),
                expr_to_column_name(idx)
            )
        }
        _ => format!("{:?}", expr),
    }
}

/// Evaluate a property map `[(key, Expr)]` into a `HashMap<String, Value>`.
///
/// Supports parameter map expansion: when a property entry has an empty key (`""`)
/// and its value is an `Expr::Parameter`, the parameter is resolved and expected to
/// be a `Value::Map`. All entries from that map are expanded into the result.
pub(crate) fn eval_prop_map(
    graph: &str,
    executor: &super::QueryExecutor<'_>,
    pairs: &[(String, Expr)],
    row: &Row,
    ctx: &FilterRow<'_>,
) -> Result<HashMap<String, Value>, GqlError> {
    let mut map = HashMap::new();
    for (key, expr) in pairs {
        if key.is_empty() {
            // Sentinel: parameter map expansion.
            let val = executor.eval_with_storage(graph, expr, row, ctx)?;
            match val {
                Value::Map(entries) => {
                    for (k, v) in entries {
                        if !v.is_null() {
                            map.insert(k.to_string(), v);
                        }
                    }
                }
                Value::Null => {
                    // Null parameter means no properties — skip.
                }
                other => {
                    return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "Map for parameter property expansion".to_string(),
                        actual: other.type_name().to_string(),
                    }));
                }
            }
        } else {
            let val = executor.eval_with_storage(graph, expr, row, ctx)?;
            // Cypher: setting a property to null = removing it (don't store nulls)
            if !val.is_null() {
                map.insert(key.clone(), val);
            }
        }
    }
    Ok(map)
}

/// Evaluate a binary operation directly on two already-evaluated `Value`s.
///
/// This handles all types including temporal, Node, Edge which cannot be
/// round-tripped through `value_to_expr` -> `eval_expr`.
pub(crate) fn eval_binary_values(
    op: crate::ast::BinaryOp,
    lv: Value,
    rv: Value,
    ctx: &FilterRow<'_>,
) -> Result<Value, GqlError> {
    use crate::ast::BinaryOp;
    // Handle list concatenation directly BEFORE the needs_direct check,
    // because Value::List may contain Node/Edge values that value_to_expr
    // would convert to Null.
    if matches!(op, BinaryOp::Add | BinaryOp::Concat) {
        if let (Value::List(mut l1), Value::List(l2)) = (lv.clone(), rv.clone()) {
            l1.extend(l2);
            return Ok(Value::List(l1));
        }
    }
    // For types that value_to_expr can represent losslessly, delegate to eval_expr.
    let needs_direct = matches!(
        (&lv, &rv),
        (Value::Date(_), _)
            | (_, Value::Date(_))
            | (Value::LocalTime(_), _)
            | (_, Value::LocalTime(_))
            | (Value::Time(_, _), _)
            | (_, Value::Time(_, _))
            | (Value::LocalDateTime(_), _)
            | (_, Value::LocalDateTime(_))
            | (Value::DateTime(_), _)
            | (_, Value::DateTime(_))
            | (Value::Duration(_), _)
            | (_, Value::Duration(_))
            | (Value::Node(_), _)
            | (_, Value::Node(_))
            | (Value::Edge(_), _)
            | (_, Value::Edge(_))
            | (Value::Path { .. }, _)
            | (_, Value::Path { .. })
    );
    if !needs_direct {
        let left_lit = value_to_expr(&lv);
        let right_lit = value_to_expr(&rv);
        let rebuilt = Expr::BinaryOp {
            op,
            left: Box::new(left_lit),
            right: Box::new(right_lit),
        };
        return eval_expr(&rebuilt, ctx);
    }
    // Handle comparison operators directly for temporal/graph types.
    match op {
        BinaryOp::Eq => Ok(match lv.cypher_eq(&rv) {
            Some(b) => Value::Bool(b),
            None => Value::Null,
        }),
        BinaryOp::Neq => Ok(match lv.cypher_eq(&rv) {
            Some(b) => Value::Bool(!b),
            None => Value::Null,
        }),
        BinaryOp::Lt | BinaryOp::Lte | BinaryOp::Gt | BinaryOp::Gte => {
            if lv.is_null() || rv.is_null() {
                return Ok(Value::Null);
            }
            // Cypher §7.2 Comparability: NaN compared with a numeric yields false;
            // NaN compared with a non-numeric (cross-type) yields null.
            // This is different from ORDER BY, where cmp_values() sorts NaN last.
            let lv_is_nan = matches!(&lv, Value::Float(f) if f.is_nan());
            let rv_is_nan = matches!(&rv, Value::Float(f) if f.is_nan());
            if lv_is_nan || rv_is_nan {
                let lv_numeric = matches!(&lv, Value::Int(_) | Value::Float(_));
                let rv_numeric = matches!(&rv, Value::Int(_) | Value::Float(_));
                return Ok(if lv_numeric && rv_numeric {
                    Value::Bool(false) // NaN comparisons between numbers are always false
                } else {
                    Value::Null // cross-type with NaN
                });
            }
            // Use Cypher comparison semantics: cross-type comparisons yield null,
            // not a total-order result.  cmp_values() is for ORDER BY (sorting only).
            let ord = match crate::executor::filter::cypher_compare(&lv, &rv) {
                Ok(ord) => ord,
                Err(_) => return Ok(Value::Null),
            };
            let result = match op {
                BinaryOp::Lt => ord == std::cmp::Ordering::Less,
                BinaryOp::Lte => ord != std::cmp::Ordering::Greater,
                BinaryOp::Gt => ord == std::cmp::Ordering::Greater,
                BinaryOp::Gte => ord != std::cmp::Ordering::Less,
                _ => unreachable!(),
            };
            Ok(Value::Bool(result))
        }
        // Temporal arithmetic: temporal +/- duration, duration +/- duration, duration * int, etc.
        BinaryOp::Add | BinaryOp::Concat => {
            use crate::executor::filter::{duration_add, temporal_add_duration};
            match (&lv, &rv) {
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (
                    temporal @ (Value::Date(_)
                    | Value::LocalTime(_)
                    | Value::Time(_, _)
                    | Value::LocalDateTime(_)
                    | Value::DateTime(_)),
                    Value::Duration(dur),
                ) => temporal_add_duration(temporal, dur, false),
                (
                    Value::Duration(dur),
                    temporal @ (Value::Date(_)
                    | Value::LocalTime(_)
                    | Value::Time(_, _)
                    | Value::LocalDateTime(_)
                    | Value::DateTime(_)),
                ) => temporal_add_duration(temporal, dur, false),
                (Value::Duration(a), Value::Duration(b)) => Ok(Value::Duration(duration_add(a, b))),
                // List concatenation: handle directly to preserve Node/Edge/Path values
                // that value_to_expr would otherwise convert to Null.
                (Value::List(l1), Value::List(l2)) => {
                    let mut result = l1.clone();
                    result.extend_from_slice(l2);
                    Ok(Value::List(result))
                }
                _ => {
                    let left_lit = value_to_expr(&lv);
                    let right_lit = value_to_expr(&rv);
                    let rebuilt = Expr::BinaryOp {
                        op,
                        left: Box::new(left_lit),
                        right: Box::new(right_lit),
                    };
                    eval_expr(&rebuilt, ctx)
                }
            }
        }
        BinaryOp::Sub => {
            use crate::executor::filter::{duration_sub, temporal_add_duration};
            match (&lv, &rv) {
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (
                    temporal @ (Value::Date(_)
                    | Value::LocalTime(_)
                    | Value::Time(_, _)
                    | Value::LocalDateTime(_)
                    | Value::DateTime(_)),
                    Value::Duration(dur),
                ) => temporal_add_duration(temporal, dur, true),
                (Value::Duration(a), Value::Duration(b)) => Ok(Value::Duration(duration_sub(a, b))),
                _ => {
                    let left_lit = value_to_expr(&lv);
                    let right_lit = value_to_expr(&rv);
                    let rebuilt = Expr::BinaryOp {
                        op,
                        left: Box::new(left_lit),
                        right: Box::new(right_lit),
                    };
                    eval_expr(&rebuilt, ctx)
                }
            }
        }
        BinaryOp::Mul => {
            use crate::executor::filter::{duration_mul, duration_mul_f64};
            match (&lv, &rv) {
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (Value::Duration(d), Value::Int(n)) => Ok(Value::Duration(duration_mul(d, *n))),
                (Value::Int(n), Value::Duration(d)) => Ok(Value::Duration(duration_mul(d, *n))),
                (Value::Duration(d), Value::Float(f)) => {
                    Ok(Value::Duration(duration_mul_f64(d, *f)))
                }
                (Value::Float(f), Value::Duration(d)) => {
                    Ok(Value::Duration(duration_mul_f64(d, *f)))
                }
                _ => {
                    let left_lit = value_to_expr(&lv);
                    let right_lit = value_to_expr(&rv);
                    let rebuilt = Expr::BinaryOp {
                        op,
                        left: Box::new(left_lit),
                        right: Box::new(right_lit),
                    };
                    eval_expr(&rebuilt, ctx)
                }
            }
        }
        BinaryOp::Div => {
            use crate::executor::filter::{duration_div, duration_mul_f64};
            match (&lv, &rv) {
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (Value::Duration(d), Value::Int(n)) => duration_div(d, *n).map(Value::Duration),
                (Value::Duration(d), Value::Float(f)) => {
                    Ok(Value::Duration(duration_mul_f64(d, 1.0 / f)))
                }
                _ => {
                    let left_lit = value_to_expr(&lv);
                    let right_lit = value_to_expr(&rv);
                    let rebuilt = Expr::BinaryOp {
                        op,
                        left: Box::new(left_lit),
                        right: Box::new(right_lit),
                    };
                    eval_expr(&rebuilt, ctx)
                }
            }
        }
        // For other ops (And, Or, In, etc.) fall through to eval_expr with best-effort conversion.
        _ => {
            let left_lit = value_to_expr(&lv);
            let right_lit = value_to_expr(&rv);
            let rebuilt = Expr::BinaryOp {
                op,
                left: Box::new(left_lit),
                right: Box::new(right_lit),
            };
            eval_expr(&rebuilt, ctx)
        }
    }
}

/// Convert a `Value` back to a literal `Expr` for re-evaluation.
///
/// Used by `eval_with_storage` to handle compound expressions where sub-parts
/// have already been evaluated against storage.
pub(crate) fn value_to_expr(val: &Value) -> Expr {
    match val {
        Value::Null => Expr::Null,
        Value::Bool(b) => Expr::Bool(*b),
        Value::Int(n) => Expr::Integer(*n),
        Value::Float(f) => Expr::Float(*f),
        Value::Str(s) => Expr::Str(s.clone()),
        Value::List(items) => Expr::List(items.iter().map(value_to_expr).collect()),
        Value::Map(m) => Expr::MapLiteral(
            m.iter()
                .map(|(k, v)| (k.as_ref().into(), value_to_expr(v)))
                .collect(),
        ),
        // Node/Edge/Path/Temporal/Spatial: return as a variable with a special sentinel.
        // These should not appear in the middle of a compound expression being rebuilt.
        _ => Expr::Null,
    }
}

/// Convert a civil time to nanoseconds-from-midnight.
pub(crate) fn civil_time_to_nanos(t: &jiff::civil::Time) -> i64 {
    t.hour() as i64 * 3_600_000_000_000
        + t.minute() as i64 * 60_000_000_000
        + t.second() as i64 * 1_000_000_000
        + t.subsec_nanosecond() as i64
}

/// Compare two `Value`s for ordering purposes.
///
/// Ordering follows openCypher/Neo4j semantics for ORDER BY / min / max.
/// Ascending order (from smallest to largest):
///   Node/Edge/Path < Map < List < Bool < Str < Number < Temporal < Null
/// Null sorts LAST in ascending order (treated as the "greatest" value).
pub fn cmp_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;

    // Type ordering (ascending, smallest to largest) per openCypher spec:
    // Map < Node < Edge < List < Path < String < Bool < Number < NaN < Null
    // Temporal types (Date, Time, DateTime, Duration) sit between Bool and Null.
    fn type_rank(v: &Value) -> u8 {
        match v {
            Value::Map(_) => 0,
            Value::Node(_) => 1,
            Value::Edge(_) => 2,
            Value::List(_) => 3,
            Value::Path { .. } => 4,
            Value::Str(_) => 5,
            Value::Bool(_) => 6,
            Value::Float(f) if f.is_nan() => 8, // NaN sorts after regular numbers
            Value::Int(_) | Value::Float(_) => 7,
            Value::Date(_) => 10,
            Value::LocalTime(_) => 11,
            Value::Time(_, _) => 11, // Zoned time sorts same rank as LocalTime
            Value::LocalDateTime(_) => 12,
            Value::DateTime(_) => 13,
            Value::Duration(_) => 14,
            Value::Point(_) => 15,
            Value::Bytes(_) => 16,
            Value::Null => 255, // Null sorts LAST (largest) in ascending order
        }
    }

    let ra = type_rank(a);
    let rb = type_rank(b);
    if ra != rb {
        return ra.cmp(&rb);
    }

    match (a, b) {
        (Value::Null, Value::Null) => Equal,
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Float(x), Value::Float(y)) => {
            // Both are either NaN (rank 8) or regular floats (rank 7).
            // If same rank, they're the same category; NaN == NaN, regular == regular.
            x.partial_cmp(y).unwrap_or(Equal)
        }
        (Value::Int(x), Value::Float(y)) => (*x as f64).partial_cmp(y).unwrap_or(Equal),
        (Value::Float(x), Value::Int(y)) => x.partial_cmp(&(*y as f64)).unwrap_or(Equal),
        (Value::Str(x), Value::Str(y)) => x.cmp(y),
        (Value::Date(x), Value::Date(y)) => x.cmp(y),
        (Value::LocalTime(x), Value::LocalTime(y)) => x.cmp(y),
        // Zoned time: compare by UTC-normalized instant
        (Value::Time(t1, o1), Value::Time(t2, o2)) => {
            let utc1 = time_to_utc_nanos(t1, *o1);
            let utc2 = time_to_utc_nanos(t2, *o2);
            utc1.cmp(&utc2)
        }
        // Mixed LocalTime / Time: LocalTime treated as-is, Time UTC-normalized
        (Value::LocalTime(t1), Value::Time(t2, o2)) => {
            let n1 = civil_time_to_nanos(t1);
            let n2 = time_to_utc_nanos(t2, *o2);
            n1.cmp(&n2)
        }
        (Value::Time(t1, o1), Value::LocalTime(t2)) => {
            let n1 = time_to_utc_nanos(t1, *o1);
            let n2 = civil_time_to_nanos(t2);
            n1.cmp(&n2)
        }
        (Value::LocalDateTime(x), Value::LocalDateTime(y)) => x.cmp(y),
        (Value::DateTime(x), Value::DateTime(y)) => x.cmp(y),
        (Value::List(x), Value::List(y)) => {
            for (a, b) in x.iter().zip(y.iter()) {
                let ord = cmp_values(a, b);
                if ord != Equal {
                    return ord;
                }
            }
            x.len().cmp(&y.len())
        }
        _ => Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOp, Identifier, Label};
    use std::cmp::Ordering;

    #[test]
    fn extract_node_id_success() {
        assert_eq!(
            extract_node_id(&Value::Node(NodeId(42)), "n").unwrap(),
            NodeId(42)
        );
    }

    #[test]
    fn extract_node_id_wrong_type() {
        assert!(extract_node_id(&Value::Int(1), "n").is_err());
    }

    #[test]
    fn inject_params_no_overwrite() {
        let mut row = Row::new();
        row.insert("$x".to_string(), Value::Int(1));
        let mut params = HashMap::new();
        params.insert("$x".to_string(), Value::Int(999));
        params.insert("$y".to_string(), Value::Int(2));
        inject_parameters(&mut row, &params);
        assert_eq!(row.get("$x"), Some(&Value::Int(1)));
        assert_eq!(row.get("$y"), Some(&Value::Int(2)));
    }

    #[test]
    fn row_key_deterministic() {
        let mut r1 = Row::new();
        r1.insert("a".to_string(), Value::Int(1));
        r1.insert("b".to_string(), Value::Str("h".into()));
        let mut r2 = Row::new();
        r2.insert("b".to_string(), Value::Str("h".into()));
        r2.insert("a".to_string(), Value::Int(1));
        assert_eq!(row_key(&r1), row_key(&r2));
    }

    #[test]
    fn eval_count_expr_valid() {
        assert_eq!(eval_count_expr(&Expr::Integer(5), &Row::new()).unwrap(), 5);
    }

    #[test]
    fn eval_count_expr_negative() {
        assert!(eval_count_expr(&Expr::Integer(-1), &Row::new()).is_err());
    }

    #[test]
    fn expr_to_col_variable() {
        assert_eq!(
            expr_to_column_name(&Expr::Variable(Identifier::new("n"))),
            "n"
        );
    }

    #[test]
    fn expr_to_col_property() {
        let expr = Expr::Property(
            Box::new(Expr::Variable(Identifier::new("n"))),
            "name".into(),
        );
        assert_eq!(expr_to_column_name(&expr), "n.name");
    }

    #[test]
    fn expr_to_col_function() {
        let expr = Expr::FunctionCall {
            name: "count".into(),
            args: vec![Expr::Variable(Identifier::new("n"))],
            distinct: false,
        };
        assert_eq!(expr_to_column_name(&expr), "count(n)");
    }

    #[test]
    fn value_to_expr_roundtrip() {
        assert_eq!(value_to_expr(&Value::Null), Expr::Null);
        assert_eq!(value_to_expr(&Value::Bool(true)), Expr::Bool(true));
        assert_eq!(value_to_expr(&Value::Int(42)), Expr::Integer(42));
    }

    #[test]
    fn discover_aliases_node_scan() {
        let op = Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        };
        assert_eq!(discover_operator_aliases(&op), vec!["n".to_string()]);
    }

    #[test]
    fn cmp_values_null_last() {
        assert_eq!(cmp_values(&Value::Null, &Value::Int(0)), Ordering::Greater);
        assert_eq!(cmp_values(&Value::Null, &Value::Null), Ordering::Equal);
    }

    #[test]
    fn cmp_values_nan_after_regular() {
        assert_eq!(
            cmp_values(&Value::Float(f64::NAN), &Value::Float(1.0)),
            Ordering::Greater
        );
    }

    #[test]
    fn cmp_values_list_lex() {
        let a = Value::List(vec![Value::Int(1), Value::Int(2)]);
        let b = Value::List(vec![Value::Int(1), Value::Int(3)]);
        assert_eq!(cmp_values(&a, &b), Ordering::Less);
    }

    #[test]
    fn eval_binary_values_list_concat() {
        let ctx: FilterRow<'_> = HashMap::new();
        let result = eval_binary_values(
            BinaryOp::Add,
            Value::List(vec![Value::Int(1)]),
            Value::List(vec![Value::Int(2)]),
            &ctx,
        )
        .unwrap();
        assert_eq!(result, Value::List(vec![Value::Int(1), Value::Int(2)]));
    }
}
