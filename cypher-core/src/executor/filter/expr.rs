//! Recursive evaluator for [`Expr`] in the GQL executor.
//!
//! Behavior preserved verbatim from the original `filter` module.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::ast::{Expr, Identifier};
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::{collections, eval_case, eval_function, operators, temporal_property_access};

/// A single result row: maps variable names to their current [`Value`].
///
/// Borrowed references throughout to avoid cloning the whole row on each call.
pub type Row<'a> = HashMap<&'a str, &'a Value>;

/// Evaluate `expr` in the context of `row`.
///
/// # Errors
///
/// Returns a [`GqlError`] if:
/// - A variable referenced in `expr` is not present in `row`
/// - A type mismatch occurs (e.g. arithmetic on a boolean)
/// - Division by zero
/// - An unsupported combination is encountered
///
/// Note: `Property` access on a Node/Edge that lacks the requested key returns
/// `Value::Null` rather than an error (Cypher semantics).
pub fn eval_expr<'r>(expr: &Expr, row: &Row<'r>) -> Result<Value, GqlError> {
    match expr {
        // ── Literals ──────────────────────────────────────────────────────────
        Expr::Integer(n) => Ok(Value::Int(*n)),
        Expr::Float(f) => Ok(Value::Float(*f)),
        Expr::Str(s) => Ok(Value::Str(s.clone())),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Null => Ok(Value::Null),

        // ── Variable lookup ───────────────────────────────────────────────────
        Expr::Variable(Identifier(name)) => match row.get(name.as_str()) {
            Some(v) => Ok((*v).clone()),
            None => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: format!("variable '{name}' in scope"),
                actual: "undefined".to_string(),
            })),
        },

        // ── Property access ───────────────────────────────────────────────────
        // Property access on a Node/Edge reference looks up the property key in
        // the actual NodeData/EdgeData.  We store full node/edge data in the row
        // as `Value::Node(id)` / `Value::Edge(id)`.  For Phase 0 the row stores
        // `Value` directly, so property access on a `Value::Map` also works.
        Expr::Property(obj_expr, key) => {
            let obj = eval_expr(obj_expr, row)?;
            match obj {
                Value::Null => Ok(Value::Null),
                Value::Map(m) if key.starts_with("~select_map:") => {
                    let mut selected = IndexMap::new();
                    for field in key["~select_map:".len()..].split(',') {
                        if let Some(value) = m.get(field) {
                            selected.insert(field.to_string().into_boxed_str(), value.clone());
                        }
                    }
                    Ok(Value::Map(selected))
                }
                Value::Map(m) => Ok(m.get(key.as_str()).cloned().unwrap_or(Value::Null)),
                // For Node/Edge references, property access returns Null in Phase 0
                // (the full executor hydrates node/edge data from storage before
                // calling eval_expr).
                Value::Node(_) | Value::Edge(_) => Ok(Value::Null),
                // Temporal property access
                Value::Date(_)
                | Value::LocalTime(_)
                | Value::Time(_, _)
                | Value::LocalDateTime(_)
                | Value::DateTime(_)
                | Value::Duration(_) => temporal_property_access(&obj, key.as_str()),
                // Point property access (x, y, z, srid, crs)
                Value::Point(_) => temporal_property_access(&obj, key.as_str()),
                _ => Ok(Value::Null),
            }
        }

        // ── Null checks ───────────────────────────────────────────────────────
        Expr::IsNull(inner) => {
            let v = eval_expr(inner, row)?;
            Ok(Value::Bool(v.is_null()))
        }
        Expr::IsNotNull(inner) => {
            let v = eval_expr(inner, row)?;
            Ok(Value::Bool(!v.is_null()))
        }

        // ── Label check (n:Label) ──────────────────────────────────────────────
        // In the basic filter evaluator, we can only check if the value is null.
        // Full label checking requires store access (done in eval_with_storage).
        Expr::HasLabel(inner, _label) => {
            let v = eval_expr(inner, row)?;
            match v {
                Value::Null => Ok(Value::Null),
                // Without store access, we can't check labels. Return true as
                // a best-effort — the engine's eval_with_storage handles this properly.
                _ => Ok(Value::Bool(true)),
            }
        }

        // ── Unary operators ───────────────────────────────────────────────────
        Expr::UnaryOp { op, expr: inner } => operators::eval_unary(*op, inner, row),

        // ── Binary operators ──────────────────────────────────────────────────
        Expr::BinaryOp { op, left, right } => operators::eval_binary(*op, left, right, row),

        // ── Function calls ────────────────────────────────────────────────────
        Expr::FunctionCall {
            name,
            args,
            distinct,
        } => eval_function(name.as_str(), args, *distinct, row),

        // ── Parameter ────────────────────────────────────────────────────────
        Expr::Parameter(name) => {
            // Parameters are resolved by the executor before calling eval_expr;
            // they should have been substituted into the row.  If still present,
            // look them up as `$name`.
            let key = format!("${name}");
            match row.get(key.as_str()) {
                Some(v) => Ok((*v).clone()),
                None => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: format!("parameter '${name}' to be bound"),
                    actual: "unbound".to_string(),
                })),
            }
        }

        // ── List literal ─────────────────────────────────────────────────────
        Expr::List(items) => {
            let mut result = Vec::with_capacity(items.len());
            for item in items {
                result.push(eval_expr(item, row)?);
            }
            Ok(Value::List(result))
        }

        // ── Map literal ──────────────────────────────────────────────────────
        Expr::MapLiteral(pairs) => {
            let mut map = indexmap::IndexMap::new();
            for (k, v) in pairs {
                let val = eval_expr(v, row)?;
                map.insert(Box::from(k.as_str()), val);
            }
            Ok(Value::Map(map))
        }

        // ── CASE expression ──────────────────────────────────────────────────
        Expr::Case {
            subject,
            alternatives,
            default,
        } => eval_case(subject.as_deref(), alternatives, default.as_deref(), row),

        // ── Subscript / slice ─────────────────────────────────────────────────
        Expr::Subscript(list_expr, index_expr) => {
            collections::eval_subscript(list_expr, index_expr, row)
        }

        Expr::Slice(list_expr, start_expr, end_expr) => {
            collections::eval_slice(list_expr, start_expr, end_expr, row)
        }

        // ── List comprehension ────────────────────────────────────────────────
        // `[variable IN list WHERE filter | map]`
        //
        // 1. Evaluate `list` → must be Value::List (or Null → return Null).
        // 2. For each element, bind `variable` in an extended row, evaluate
        //    the optional `filter`; skip elements where filter is false/null.
        // 3. Evaluate `map` for each surviving element with `variable` bound.
        // 4. Return Value::List of mapped values.
        Expr::ListComp {
            variable,
            list,
            filter,
            map,
        } => collections::eval_list_comp(variable, list, filter, map, row),

        // ── Reduce expression ────────────────────────────────────────────────
        Expr::Reduce {
            accumulator,
            initial,
            variable,
            list,
            expression,
        } => collections::eval_reduce(accumulator, initial, variable, list, expression, row),

        // ── Existential subquery / pattern expression ─────────────────────────
        Expr::ExistsSubquery { .. } | Expr::PatternExpr(_) => Err(GqlError::Execution(
            ExecutionError::unsupported("existential subquery / pattern expression"),
        )),
    }
}
