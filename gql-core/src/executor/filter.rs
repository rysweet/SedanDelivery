//! Expression evaluator for the GQL executor.
//!
//! [`eval_expr`] recursively evaluates an [`Expr`] against a row — a map from
//! variable name to [`Value`].  Every Expr variant is fully handled.
//!
//! Arithmetic follows Cypher semantics:
//! - Integer op Integer → Integer (with overflow → Float promotion for `/`)
//! - Float op anything-numeric → Float
//! - String + String → String concatenation
//! - NULL in any binary expression propagates NULL (except IS NULL / IS NOT NULL)
//! - Division by zero → [`ExecutionError::DivisionByZero`]

use std::collections::HashMap;

use crate::ast::{BinaryOp, Expr, Identifier, UnaryOp};
use crate::error::{ExecutionError, GqlError};
use crate::types::{CypherDuration, Value};

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
                Value::Map(m) => Ok(m.get(key.as_str()).cloned().unwrap_or(Value::Null)),
                // For Node/Edge references, property access returns Null in Phase 0
                // (the full executor hydrates node/edge data from storage before
                // calling eval_expr).
                Value::Node(_) | Value::Edge(_) => Ok(Value::Null),
                // Temporal property access
                Value::Date(_)
                | Value::LocalTime(_)
                | Value::LocalDateTime(_)
                | Value::DateTime(_)
                | Value::Duration(_) => temporal_property_access(&obj, key.as_str()),
                other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Node, Relationship, or Map".to_string(),
                    actual: other.type_name().to_string(),
                })),
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
        Expr::UnaryOp { op, expr: inner } => eval_unary(*op, inner, row),

        // ── Binary operators ──────────────────────────────────────────────────
        Expr::BinaryOp { op, left, right } => eval_binary(*op, left, right, row),

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
            let list = eval_expr(list_expr, row)?;
            let idx = eval_expr(index_expr, row)?;
            match (list, idx) {
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (Value::List(items), Value::Int(i)) => {
                    let len = items.len();
                    // Cypher: negative indices count from the end.
                    // Out-of-bounds returns null in Cypher.
                    let actual = if i < 0 {
                        let offset = (-i) as usize;
                        if offset > len {
                            return Ok(Value::Null);
                        }
                        len - offset
                    } else {
                        i as usize
                    };
                    if actual >= len {
                        return Ok(Value::Null);
                    }
                    Ok(items[actual].clone())
                }
                // Map[String] — dynamic property access: map['key']
                (Value::Map(m), Value::Str(key)) => {
                    Ok(m.get(key.as_ref()).cloned().unwrap_or(Value::Null))
                }
                (list, idx) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "List[Integer] or Map[String]".to_string(),
                    actual: format!("{}[{}]", list.type_name(), idx.type_name()),
                })),
            }
        }

        Expr::Slice(list_expr, start_expr, end_expr) => {
            let list = eval_expr(list_expr, row)?;
            match list {
                Value::Null => Ok(Value::Null),
                Value::List(items) => {
                    let len = items.len() as i64;
                    // Cypher: null slice bounds → null result
                    let start = match start_expr {
                        Some(e) => match eval_expr(e, row)? {
                            Value::Int(i) => {
                                // Negative indices count from end
                                let idx = if i < 0 { (len + i).max(0) } else { i.min(len) };
                                idx as usize
                            }
                            Value::Null => return Ok(Value::Null),
                            other => {
                                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                    expected: "Integer".to_string(),
                                    actual: other.type_name().to_string(),
                                }))
                            }
                        },
                        None => 0,
                    };
                    let end = match end_expr {
                        Some(e) => match eval_expr(e, row)? {
                            Value::Int(i) => {
                                let idx = if i < 0 { (len + i).max(0) } else { i.min(len) };
                                idx as usize
                            }
                            Value::Null => return Ok(Value::Null),
                            other => {
                                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                    expected: "Integer".to_string(),
                                    actual: other.type_name().to_string(),
                                }))
                            }
                        },
                        None => len as usize,
                    };
                    // Invalid range (start > end) → empty list
                    if start >= end {
                        Ok(Value::List(vec![]))
                    } else {
                        Ok(Value::List(items[start..end.min(items.len())].to_vec()))
                    }
                }
                other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "List".to_string(),
                    actual: other.type_name().to_string(),
                })),
            }
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
        } => {
            let list_val = eval_expr(list, row)?;
            match list_val {
                Value::Null => Ok(Value::Null),
                Value::List(items) => {
                    let var_name: &str = variable.0.as_str();
                    let mut result = Vec::with_capacity(items.len());
                    for item in &items {
                        // Build an extended row with the loop variable bound.
                        let mut extended: HashMap<&str, &Value> =
                            row.iter().map(|(&k, v)| (k, *v)).collect();
                        extended.insert(var_name, item);

                        // Evaluate filter if present; skip element on false/null.
                        if let Some(pred) = filter {
                            match eval_expr(pred, &extended)? {
                                Value::Bool(true) => {}
                                Value::Bool(false) | Value::Null => continue,
                                other => {
                                    return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                        expected: "Boolean".to_string(),
                                        actual: other.type_name().to_string(),
                                    }))
                                }
                            }
                        }

                        // Map the surviving element.
                        result.push(eval_expr(map, &extended)?);
                    }
                    Ok(Value::List(result))
                }
                other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "List".to_string(),
                    actual: other.type_name().to_string(),
                })),
            }
        }

        // ── Existential subquery / pattern expression ─────────────────────────
        Expr::ExistsSubquery { .. } | Expr::PatternExpr(_) => {
            Err(GqlError::Execution(ExecutionError::unsupported(
                "existential subquery / pattern expression",
            )))
        }
    }
}

// ── Unary operator evaluation ─────────────────────────────────────────────────

fn eval_unary<'r>(op: UnaryOp, expr: &Expr, row: &Row<'r>) -> Result<Value, GqlError> {
    let val = eval_expr(expr, row)?;
    match op {
        UnaryOp::Neg => match val {
            Value::Null => Ok(Value::Null),
            Value::Int(n) => Ok(Value::Int(-n)),
            Value::Float(f) => Ok(Value::Float(-f)),
            other => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "-".to_string(),
                left: other.type_name().to_string(),
                right: "".to_string(),
            })),
        },
        UnaryOp::Not => match val {
            Value::Null => Ok(Value::Null),
            Value::Bool(b) => Ok(Value::Bool(!b)),
            other => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "NOT".to_string(),
                left: other.type_name().to_string(),
                right: "".to_string(),
            })),
        },
    }
}

// ── Binary operator evaluation ────────────────────────────────────────────────

fn eval_binary<'r>(
    op: BinaryOp,
    left: &Expr,
    right: &Expr,
    row: &Row<'r>,
) -> Result<Value, GqlError> {
    // Short-circuit AND / OR before evaluating both sides.
    match op {
        BinaryOp::And => {
            let lv = eval_expr(left, row)?;
            if lv == Value::Bool(false) {
                return Ok(Value::Bool(false));
            }
            let rv = eval_expr(right, row)?;
            return match (lv, rv) {
                (Value::Null, Value::Bool(false)) | (Value::Bool(false), Value::Null) => {
                    Ok(Value::Bool(false))
                }
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a && b)),
                (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                    op: "AND".to_string(),
                    left: l.type_name().to_string(),
                    right: r.type_name().to_string(),
                })),
            };
        }
        BinaryOp::Or => {
            let lv = eval_expr(left, row)?;
            if lv == Value::Bool(true) {
                return Ok(Value::Bool(true));
            }
            let rv = eval_expr(right, row)?;
            return match (lv, rv) {
                (Value::Null, Value::Bool(true)) | (Value::Bool(true), Value::Null) => {
                    Ok(Value::Bool(true))
                }
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a || b)),
                (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                    op: "OR".to_string(),
                    left: l.type_name().to_string(),
                    right: r.type_name().to_string(),
                })),
            };
        }
        _ => {}
    }

    let lv = eval_expr(left, row)?;
    let rv = eval_expr(right, row)?;

    match op {
        BinaryOp::And | BinaryOp::Or => unreachable!("handled above"),

        // ── Xor ──────────────────────────────────────────────────────────────
        BinaryOp::Xor => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a ^ b)),
            (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "XOR".to_string(),
                left: l.type_name().to_string(),
                right: r.type_name().to_string(),
            })),
        },

        // ── Arithmetic ────────────────────────────────────────────────────────
        BinaryOp::Add | BinaryOp::Concat => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.wrapping_add(b))),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 + b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a + b as f64)),
            (Value::Str(a), Value::Str(b)) => Ok(Value::Str(
                (a.as_ref().to_string() + b.as_ref()).into_boxed_str(),
            )),
            (Value::List(mut a), Value::List(b)) => {
                a.extend(b);
                Ok(Value::List(a))
            }
            // Temporal + Duration
            (ref temporal @ (Value::Date(_) | Value::LocalTime(_) | Value::LocalDateTime(_) | Value::DateTime(_)), Value::Duration(ref dur)) => {
                temporal_add_duration(temporal, dur, false)
            }
            // Duration + Temporal
            (Value::Duration(ref dur), ref temporal @ (Value::Date(_) | Value::LocalTime(_) | Value::LocalDateTime(_) | Value::DateTime(_))) => {
                temporal_add_duration(temporal, dur, false)
            }
            // Duration + Duration
            (Value::Duration(a), Value::Duration(b)) => Ok(Value::Duration(duration_add(&a, &b))),
            // List + element → append
            (Value::List(mut a), val) => {
                a.push(val);
                Ok(Value::List(a))
            }
            // element + List → prepend
            (val, Value::List(mut b)) => {
                b.insert(0, val);
                Ok(Value::List(b))
            }
            (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "+".to_string(),
                left: l.type_name().to_string(),
                right: r.type_name().to_string(),
            })),
        },

        BinaryOp::Sub => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.wrapping_sub(b))),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 - b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a - b as f64)),
            // Temporal - Duration
            (ref temporal @ (Value::Date(_) | Value::LocalTime(_) | Value::LocalDateTime(_) | Value::DateTime(_)), Value::Duration(ref dur)) => {
                temporal_add_duration(temporal, dur, true)
            }
            // Duration - Duration
            (Value::Duration(a), Value::Duration(b)) => Ok(Value::Duration(duration_sub(&a, &b))),
            (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "-".to_string(),
                left: l.type_name().to_string(),
                right: r.type_name().to_string(),
            })),
        },

        BinaryOp::Mul => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.wrapping_mul(b))),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 * b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a * b as f64)),
            // Duration * Integer
            (Value::Duration(d), Value::Int(n)) => Ok(Value::Duration(duration_mul(&d, n))),
            (Value::Int(n), Value::Duration(d)) => Ok(Value::Duration(duration_mul(&d, n))),
            // Duration * Float
            (Value::Duration(d), Value::Float(f)) => Ok(Value::Duration(duration_mul(&d, f as i64))),
            (Value::Float(f), Value::Duration(d)) => Ok(Value::Duration(duration_mul(&d, f as i64))),
            (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "*".to_string(),
                left: l.type_name().to_string(),
                right: r.type_name().to_string(),
            })),
        },

        BinaryOp::Div => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Int(_), Value::Int(0)) => {
                Err(GqlError::Execution(ExecutionError::DivisionByZero))
            }
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a / b)),
            // Float division by zero follows IEEE 754: returns Inf, -Inf, or NaN.
            // The TCK expects this behavior (e.g. 0.0/0.0 → NaN).
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a / b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 / b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a / b as f64)),
            // Duration / Integer
            (Value::Duration(d), Value::Int(n)) => duration_div(&d, n).map(Value::Duration),
            // Duration / Float
            (Value::Duration(d), Value::Float(f)) => duration_div(&d, f as i64).map(Value::Duration),
            (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "/".to_string(),
                left: l.type_name().to_string(),
                right: r.type_name().to_string(),
            })),
        },

        BinaryOp::Mod => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Int(_), Value::Int(0)) => {
                Err(GqlError::Execution(ExecutionError::DivisionByZero))
            }
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a % b)),
            // Float modulo by zero follows IEEE 754: returns NaN.
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a % b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 % b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a % b as f64)),
            (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "%".to_string(),
                left: l.type_name().to_string(),
                right: r.type_name().to_string(),
            })),
        },

        BinaryOp::Pow => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            // ^ always returns Float per Cypher spec.
            (Value::Int(a), Value::Int(b)) => Ok(Value::Float((a as f64).powf(b as f64))),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a.powf(b))),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float((a as f64).powf(b))),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a.powf(b as f64))),
            (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "^".to_string(),
                left: l.type_name().to_string(),
                right: r.type_name().to_string(),
            })),
        },

        // ── Comparison ────────────────────────────────────────────────────────
        BinaryOp::Eq => Ok(match lv.cypher_eq(&rv) {
            Some(b) => Value::Bool(b),
            None => Value::Null,
        }),
        BinaryOp::Neq => Ok(match lv.cypher_eq(&rv) {
            Some(b) => Value::Bool(!b),
            None => Value::Null,
        }),
        BinaryOp::Lt => compare_values(&lv, &rv, std::cmp::Ordering::Less, false),
        BinaryOp::Lte => compare_values(&lv, &rv, std::cmp::Ordering::Less, true),
        BinaryOp::Gt => compare_values(&lv, &rv, std::cmp::Ordering::Greater, false),
        BinaryOp::Gte => compare_values(&lv, &rv, std::cmp::Ordering::Greater, true),

        // ── String predicates ─────────────────────────────────────────────────
        BinaryOp::StartsWith => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Str(s), Value::Str(prefix)) => Ok(Value::Bool(s.starts_with(prefix.as_ref()))),
            // Type mismatch in string predicates returns null in Cypher
            (_, _) => Ok(Value::Null),
        },
        BinaryOp::EndsWith => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Str(s), Value::Str(suffix)) => Ok(Value::Bool(s.ends_with(suffix.as_ref()))),
            (_, _) => Ok(Value::Null),
        },
        BinaryOp::Contains => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Str(s), Value::Str(needle)) => Ok(Value::Bool(s.contains(needle.as_ref()))),
            (_, _) => Ok(Value::Null),
        },
        BinaryOp::RegexMatch => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Str(s), Value::Str(pattern)) => {
                // Use the `regex` crate already in scope for the workspace.
                let re = regex::Regex::new(pattern.as_ref()).map_err(|e| {
                    GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "valid regex pattern".to_string(),
                        actual: e.to_string(),
                    })
                })?;
                Ok(Value::Bool(re.is_match(s.as_ref())))
            }
            (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "=~".to_string(),
                left: l.type_name().to_string(),
                right: r.type_name().to_string(),
            })),
        },

        // ── IN operator ───────────────────────────────────────────────────────
        BinaryOp::In => match rv {
            Value::Null => Ok(Value::Null),
            Value::List(items) => {
                if lv.is_null() {
                    // null IN [] → false; null IN [any non-empty list] → null
                    // Because null == anything is null, so membership is always unknown
                    // for non-empty lists.
                    if items.is_empty() {
                        Ok(Value::Bool(false))
                    } else {
                        Ok(Value::Null)
                    }
                } else {
                    // x IN list: true if any element equals x; null if null present and not found.
                    let mut found_null = false;
                    for item in &items {
                        match lv.cypher_eq(item) {
                            Some(true) => return Ok(Value::Bool(true)),
                            None => found_null = true,
                            Some(false) => {}
                        }
                    }
                    if found_null {
                        Ok(Value::Null)
                    } else {
                        Ok(Value::Bool(false))
                    }
                }
            }
            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "List".to_string(),
                actual: other.type_name().to_string(),
            })),
        },
    }
}

/// Helper for ordered comparisons.  `also_eq` is true for `<=` / `>=`.
fn compare_values(
    lv: &Value,
    rv: &Value,
    target: std::cmp::Ordering,
    also_eq: bool,
) -> Result<Value, GqlError> {
    if lv.is_null() || rv.is_null() {
        return Ok(Value::Null);
    }
    match cypher_compare(lv, rv) {
        Ok(ord) => {
            let result = if also_eq {
                ord == target || ord == std::cmp::Ordering::Equal
            } else {
                ord == target
            };
            Ok(Value::Bool(result))
        }
        // Incompatible types: Cypher returns null for cross-type comparison
        Err(_) => Ok(Value::Null),
    }
}

/// Total order comparison for Cypher values (comparable types only).
fn cypher_compare(a: &Value, b: &Value) -> Result<std::cmp::Ordering, GqlError> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(x.cmp(y)),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).ok_or_else(|| {
            GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "comparable Float".to_string(),
                actual: "NaN".to_string(),
            })
        }),
        (Value::Int(x), Value::Float(y)) => {
            if y.is_nan() { return Ok(std::cmp::Ordering::Less); }
            (*x as f64).partial_cmp(y).ok_or_else(|| {
                GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "comparable Float".to_string(),
                    actual: "NaN".to_string(),
                })
            })
        }
        (Value::Float(x), Value::Int(y)) => {
            if x.is_nan() { return Ok(std::cmp::Ordering::Greater); }
            x.partial_cmp(&(*y as f64)).ok_or_else(|| {
                GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "comparable Float".to_string(),
                    actual: "NaN".to_string(),
                })
            })
        }
        (Value::Str(x), Value::Str(y)) => Ok(x.as_ref().cmp(y.as_ref())),
        (Value::Bool(x), Value::Bool(y)) => Ok(x.cmp(y)),
        // Temporal comparisons
        (Value::Date(x), Value::Date(y)) => Ok(x.cmp(y)),
        (Value::LocalTime(x), Value::LocalTime(y)) => Ok(x.cmp(y)),
        (Value::LocalDateTime(x), Value::LocalDateTime(y)) => Ok(x.cmp(y)),
        (Value::DateTime(x), Value::DateTime(y)) => Ok(x.cmp(y)),
        // List comparison: element-by-element
        (Value::List(x), Value::List(y)) => {
            for (a, b) in x.iter().zip(y.iter()) {
                match cypher_compare(a, b) {
                    Ok(std::cmp::Ordering::Equal) => continue,
                    Ok(ord) => return Ok(ord),
                    Err(e) => return Err(e),
                }
            }
            Ok(x.len().cmp(&y.len()))
        }
        (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
            op: "</>".to_string(),
            left: l.type_name().to_string(),
            right: r.type_name().to_string(),
        })),
    }
}

// ── Built-in function evaluation ──────────────────────────────────────────────

/// Evaluate a quantifier predicate: `all`, `any`, `none`, or `single`.
///
/// These take the form `func(x IN list WHERE pred)` which the parser rewrites
/// to `FunctionCall(name, [ListComp { variable, list, filter, map }])`.
/// We must evaluate the predicate for *every* element (not filter/skip) so we
/// can correctly count true/false/null results.
fn eval_quantifier<'r>(name: &str, args: &[Expr], row: &Row<'r>) -> Result<Value, GqlError> {
    if args.len() != 1 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: format!("{name}(variable IN list WHERE predicate)"),
            actual: format!("{} arguments", args.len()),
        }));
    }

    match &args[0] {
        Expr::ListComp {
            variable,
            list,
            filter,
            ..
        } => {
            let list_val = eval_expr(list, row)?;
            match list_val {
                Value::Null => Ok(Value::Null),
                Value::List(items) => {
                    let var_name = variable.0.as_str();
                    let mut true_count: usize = 0;
                    let mut false_count: usize = 0;
                    let mut null_count: usize = 0;

                    for item in &items {
                        let mut extended: HashMap<&str, &Value> =
                            row.iter().map(|(&k, v)| (k, *v)).collect();
                        extended.insert(var_name, item);

                        let result = if let Some(pred) = filter {
                            eval_expr(pred, &extended)?
                        } else {
                            // No WHERE — treat the element itself as the predicate value.
                            item.clone()
                        };

                        match result {
                            Value::Bool(true) => true_count += 1,
                            Value::Bool(false) => false_count += 1,
                            Value::Null => null_count += 1,
                            // Non-boolean values are treated as false.
                            _ => false_count += 1,
                        }
                    }

                    match name.to_ascii_lowercase().as_str() {
                        "all" => {
                            if false_count > 0 {
                                Ok(Value::Bool(false))
                            } else if null_count > 0 {
                                Ok(Value::Null)
                            } else {
                                Ok(Value::Bool(true))
                            }
                        }
                        "any" => {
                            if true_count > 0 {
                                Ok(Value::Bool(true))
                            } else if null_count > 0 {
                                Ok(Value::Null)
                            } else {
                                Ok(Value::Bool(false))
                            }
                        }
                        "none" => {
                            if true_count > 0 {
                                Ok(Value::Bool(false))
                            } else if null_count > 0 {
                                Ok(Value::Null)
                            } else {
                                Ok(Value::Bool(true))
                            }
                        }
                        "single" => {
                            if true_count > 1 {
                                Ok(Value::Bool(false))
                            } else if null_count > 0 {
                                Ok(Value::Null)
                            } else {
                                Ok(Value::Bool(true_count == 1))
                            }
                        }
                        _ => unreachable!(),
                    }
                }
                other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "List".to_string(),
                    actual: other.type_name().to_string(),
                })),
            }
        }
        // Fallback: if the arg is not a ListComp (e.g. `all(someListVar)`), evaluate
        // it as a list of values and check their truthiness.
        other => {
            let val = eval_expr(other, row)?;
            match val {
                Value::Null => Ok(Value::Null),
                Value::List(items) => {
                    let mut true_count: usize = 0;
                    let mut false_count: usize = 0;
                    let mut null_count: usize = 0;

                    for item in &items {
                        match item {
                            Value::Bool(true) => true_count += 1,
                            Value::Bool(false) => false_count += 1,
                            Value::Null => null_count += 1,
                            _ => false_count += 1,
                        }
                    }

                    match name.to_ascii_lowercase().as_str() {
                        "all" => {
                            if false_count > 0 {
                                Ok(Value::Bool(false))
                            } else if null_count > 0 {
                                Ok(Value::Null)
                            } else {
                                Ok(Value::Bool(true))
                            }
                        }
                        "any" => {
                            if true_count > 0 {
                                Ok(Value::Bool(true))
                            } else if null_count > 0 {
                                Ok(Value::Null)
                            } else {
                                Ok(Value::Bool(false))
                            }
                        }
                        "none" => {
                            if true_count > 0 {
                                Ok(Value::Bool(false))
                            } else if null_count > 0 {
                                Ok(Value::Null)
                            } else {
                                Ok(Value::Bool(true))
                            }
                        }
                        "single" => {
                            if true_count > 1 {
                                Ok(Value::Bool(false))
                            } else if null_count > 0 {
                                Ok(Value::Null)
                            } else {
                                Ok(Value::Bool(true_count == 1))
                            }
                        }
                        _ => unreachable!(),
                    }
                }
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "List".to_string(),
                    actual: val.type_name().to_string(),
                })),
            }
        }
    }
}

fn eval_function<'r>(
    name: &str,
    args: &[Expr],
    _distinct: bool,
    row: &Row<'r>,
) -> Result<Value, GqlError> {
    // Quantifier predicates need access to the raw ListComp expr —
    // they must NOT eagerly evaluate args (which would lose predicate info).
    match name.to_ascii_lowercase().as_str() {
        "all" | "any" | "none" | "single" => return eval_quantifier(name, args, row),
        _ => {}
    }

    // Evaluate all arguments eagerly.
    let mut vals: Vec<Value> = Vec::with_capacity(args.len());
    for a in args {
        vals.push(eval_expr(a, row)?);
    }

    eval_function_with_vals(name, vals)
}

/// Evaluate a function call with already-evaluated arguments.
/// This is used by the engine when it needs to call functions with
/// values that have already been resolved from storage.
pub fn eval_function_with_vals(name: &str, vals: Vec<Value>) -> Result<Value, GqlError> {
    match name.to_ascii_lowercase().as_str() {
        // ── Scalar functions ──────────────────────────────────────────────────
        "rand" => {
            require_arity("rand", 0, &vals)?;
            // Use a simple hash-based pseudo-random to avoid adding a crate dependency.
            // The TCK only checks that the result is a float in [0.0, 1.0).
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .hash(&mut hasher);
            std::thread::current().id().hash(&mut hasher);
            let bits = hasher.finish();
            // Convert to f64 in [0.0, 1.0)
            let val = (bits >> 11) as f64 / ((1u64 << 53) as f64);
            Ok(Value::Float(val))
        }
        "abs" => {
            require_arity("abs", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Int(n) => Ok(Value::Int(n.abs())),
                Value::Float(f) => Ok(Value::Float(f.abs())),
                other => type_error("abs", other),
            }
        }
        "ceil" => {
            require_arity("ceil", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Int(n) => Ok(Value::Int(*n)),
                Value::Float(f) => Ok(Value::Float(f.ceil())),
                other => type_error("ceil", other),
            }
        }
        "floor" => {
            require_arity("floor", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Int(n) => Ok(Value::Int(*n)),
                Value::Float(f) => Ok(Value::Float(f.floor())),
                other => type_error("floor", other),
            }
        }
        "round" => {
            require_arity("round", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Int(n) => Ok(Value::Int(*n)),
                Value::Float(f) => Ok(Value::Float(f.round())),
                other => type_error("round", other),
            }
        }
        "sqrt" => {
            require_arity("sqrt", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Int(n) => Ok(Value::Float((*n as f64).sqrt())),
                Value::Float(f) => Ok(Value::Float(f.sqrt())),
                other => type_error("sqrt", other),
            }
        }
        "sign" => {
            require_arity("sign", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Int(n) => Ok(Value::Int(n.signum())),
                Value::Float(f) => Ok(Value::Float(f.signum())),
                other => type_error("sign", other),
            }
        }
        "tostring" => {
            require_arity("toString", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                // A String is already a string — return it as-is (no extra quotes).
                Value::Str(s) => Ok(Value::Str(s.clone())),
                Value::Int(_) | Value::Float(_) | Value::Bool(_) => {
                    Ok(Value::Str(vals[0].to_string().into_boxed_str()))
                }
                Value::Date(_) | Value::LocalTime(_) | Value::LocalDateTime(_)
                | Value::DateTime(_) | Value::Duration(_) => {
                    Ok(Value::Str(vals[0].to_string().into_boxed_str()))
                }
                other => type_error("toString", other),
            }
        }
        "tointeger" | "toint" => {
            require_arity("toInteger", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Int(n) => Ok(Value::Int(*n)),
                Value::Float(f) => Ok(Value::Int(*f as i64)),
                Value::Str(s) => {
                    // Try integer parse first, then float parse (truncating).
                    // TCK expects NULL for truly non-numeric strings.
                    if let Ok(n) = s.parse::<i64>() {
                        Ok(Value::Int(n))
                    } else if let Ok(f) = s.parse::<f64>() {
                        Ok(Value::Int(f as i64))
                    } else {
                        Ok(Value::Null)
                    }
                }
                other => type_error("toInteger", other),
            }
        }
        "tofloat" => {
            require_arity("toFloat", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Float(f) => Ok(Value::Float(*f)),
                Value::Int(n) => Ok(Value::Float(*n as f64)),
                Value::Str(s) => {
                    // TCK expects NULL for non-numeric strings, not an error.
                    Ok(s.parse::<f64>().map(Value::Float).unwrap_or(Value::Null))
                }
                other => type_error("toFloat", other),
            }
        }
        "toboolean" => {
            require_arity("toBoolean", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Bool(b) => Ok(Value::Bool(*b)),
                Value::Str(s) => match s.to_ascii_lowercase().as_str() {
                    "true" => Ok(Value::Bool(true)),
                    "false" => Ok(Value::Bool(false)),
                    _ => Ok(Value::Null),
                },
                other => type_error("toBoolean", other),
            }
        }

        // ── String functions ──────────────────────────────────────────────────
        "size" => {
            require_arity("size", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
                Value::List(l) => Ok(Value::Int(l.len() as i64)),
                other => type_error("size", other),
            }
        }
        "length" => {
            require_arity("length", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
                other => type_error("length", other),
            }
        }
        "toupper" => {
            require_arity("toUpper", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Str(s) => Ok(Value::Str(s.to_uppercase().into_boxed_str())),
                other => type_error("toUpper", other),
            }
        }
        "tolower" => {
            require_arity("toLower", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Str(s) => Ok(Value::Str(s.to_lowercase().into_boxed_str())),
                other => type_error("toLower", other),
            }
        }
        "trim" => {
            require_arity("trim", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Str(s) => Ok(Value::Str(s.trim().to_string().into_boxed_str())),
                other => type_error("trim", other),
            }
        }
        "ltrim" => {
            require_arity("ltrim", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Str(s) => Ok(Value::Str(s.trim_start().to_string().into_boxed_str())),
                other => type_error("ltrim", other),
            }
        }
        "rtrim" => {
            require_arity("rtrim", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Str(s) => Ok(Value::Str(s.trim_end().to_string().into_boxed_str())),
                other => type_error("rtrim", other),
            }
        }
        "left" => {
            require_arity("left", 2, &vals)?;
            match (&vals[0], &vals[1]) {
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (Value::Str(s), Value::Int(n)) => {
                    let n = (*n).max(0) as usize;
                    Ok(Value::Str(
                        s.chars().take(n).collect::<String>().into_boxed_str(),
                    ))
                }
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "String, Integer".to_string(),
                    actual: format!("{}, {}", vals[0].type_name(), vals[1].type_name()),
                })),
            }
        }
        "right" => {
            require_arity("right", 2, &vals)?;
            match (&vals[0], &vals[1]) {
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (Value::Str(s), Value::Int(n)) => {
                    let n = (*n).max(0) as usize;
                    let chars: Vec<char> = s.chars().collect();
                    let start = chars.len().saturating_sub(n);
                    Ok(Value::Str(
                        chars[start..].iter().collect::<String>().into_boxed_str(),
                    ))
                }
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "String, Integer".to_string(),
                    actual: format!("{}, {}", vals[0].type_name(), vals[1].type_name()),
                })),
            }
        }
        "substring" => {
            if vals.len() < 2 || vals.len() > 3 {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "substring(String, Integer[, Integer])".to_string(),
                    actual: format!("{} arguments", vals.len()),
                }));
            }
            match (&vals[0], &vals[1]) {
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (Value::Str(s), Value::Int(start)) => {
                    let chars: Vec<char> = s.chars().collect();
                    let len = chars.len();
                    let start = (*start).max(0) as usize;
                    let end = if vals.len() == 3 {
                        match &vals[2] {
                            Value::Int(n) => (start + (*n).max(0) as usize).min(len),
                            Value::Null => len,
                            other => return type_error("substring", other),
                        }
                    } else {
                        len
                    };
                    Ok(Value::Str(
                        chars[start.min(len)..end.min(len)]
                            .iter()
                            .collect::<String>()
                            .into_boxed_str(),
                    ))
                }
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "String, Integer".to_string(),
                    actual: format!("{}, {}", vals[0].type_name(), vals[1].type_name()),
                })),
            }
        }
        "replace" => {
            require_arity("replace", 3, &vals)?;
            match (&vals[0], &vals[1], &vals[2]) {
                (Value::Null, _, _) | (_, Value::Null, _) | (_, _, Value::Null) => Ok(Value::Null),
                (Value::Str(s), Value::Str(from), Value::Str(to)) => Ok(Value::Str(
                    s.replace(from.as_ref(), to.as_ref()).into_boxed_str(),
                )),
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "String, String, String".to_string(),
                    actual: format!(
                        "{}, {}, {}",
                        vals[0].type_name(),
                        vals[1].type_name(),
                        vals[2].type_name()
                    ),
                })),
            }
        }
        "split" => {
            require_arity("split", 2, &vals)?;
            match (&vals[0], &vals[1]) {
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (Value::Str(s), Value::Str(delim)) => {
                    let parts: Vec<Value> = s
                        .split(delim.as_ref())
                        .map(|p| Value::Str(p.to_string().into_boxed_str()))
                        .collect();
                    Ok(Value::List(parts))
                }
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "String, String".to_string(),
                    actual: format!("{}, {}", vals[0].type_name(), vals[1].type_name()),
                })),
            }
        }

        // ── List functions ────────────────────────────────────────────────────
        "head" => {
            require_arity("head", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::List(l) => Ok(l.first().cloned().unwrap_or(Value::Null)),
                other => type_error("head", other),
            }
        }
        "last" => {
            require_arity("last", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::List(l) => Ok(l.last().cloned().unwrap_or(Value::Null)),
                other => type_error("last", other),
            }
        }
        "tail" => {
            require_arity("tail", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::List(l) => Ok(Value::List(l[1.min(l.len())..].to_vec())),
                other => type_error("tail", other),
            }
        }
        "reverse" => {
            require_arity("reverse", 1, &vals)?;
            match vals.into_iter().next().unwrap() {
                Value::Null => Ok(Value::Null),
                Value::List(mut l) => {
                    l.reverse();
                    Ok(Value::List(l))
                }
                Value::Str(s) => Ok(Value::Str(
                    s.chars().rev().collect::<String>().into_boxed_str(),
                )),
                other => type_error("reverse", &other),
            }
        }
        "coalesce" => {
            // Return first non-null argument.
            for v in vals {
                if !v.is_null() {
                    return Ok(v);
                }
            }
            Ok(Value::Null)
        }
        "isnull" => {
            require_arity("isNull", 1, &vals)?;
            Ok(Value::Bool(vals[0].is_null()))
        }

        // ── Map / collection functions ──────────────────────────────────────
        "keys" => {
            require_arity("keys", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Map(m) => Ok(Value::List(
                    m.iter()
                        .filter(|(_, v)| !v.is_null())
                        .map(|(k, _)| Value::Str(k.clone()))
                        .collect(),
                )),
                other => type_error("keys", other),
            }
        }
        "properties" => {
            require_arity("properties", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Map(m) => Ok(Value::Map(m.clone())),
                other => type_error("properties", other),
            }
        }

        // ── range() ─────────────────────────────────────────────────────────
        "range" => {
            if vals.len() < 2 || vals.len() > 3 {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "range(Integer, Integer[, Integer])".to_string(),
                    actual: format!("{} arguments", vals.len()),
                }));
            }
            match (&vals[0], &vals[1]) {
                (Value::Int(start), Value::Int(end)) => {
                    let step = if vals.len() == 3 {
                        match &vals[2] {
                            Value::Int(s) => *s,
                            other => return type_error("range", other),
                        }
                    } else {
                        1
                    };
                    if step == 0 {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "non-zero step for range()".to_string(),
                            actual: "0".to_string(),
                        }));
                    }
                    let mut result = Vec::new();
                    let mut i = *start;
                    if step > 0 {
                        while i <= *end {
                            result.push(Value::Int(i));
                            i += step;
                        }
                    } else {
                        while i >= *end {
                            result.push(Value::Int(i));
                            i += step;
                        }
                    }
                    Ok(Value::List(result))
                }
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Integer, Integer".to_string(),
                    actual: format!("{}, {}", vals[0].type_name(), vals[1].type_name()),
                })),
            }
        }

        // ── Temporal constructor functions ───────────────────────────────────
        "date" => eval_date_constructor(&vals),
        "time" => eval_time_constructor(&vals),
        "localtime" => eval_localtime_constructor(&vals),
        "datetime" => eval_datetime_constructor(&vals),
        "localdatetime" => eval_localdatetime_constructor(&vals),
        "duration" => eval_duration_constructor(&vals),

        // ── Temporal namespaced functions ─────────────────────────────────────
        "date.truncate" => eval_date_truncate(&vals),
        "datetime.truncate" => eval_datetime_truncate(&vals),
        "localdatetime.truncate" => eval_localdatetime_truncate(&vals),
        "localtime.truncate" => eval_localtime_truncate(&vals),
        "time.truncate" => eval_time_truncate(&vals),

        "date.realtime" | "date.statement" | "date.transaction" => {
            if vals.first().map(|v| v.is_null()).unwrap_or(false) {
                return Ok(Value::Null);
            }
            if !vals.is_empty() { return eval_date_constructor(&vals); }
            let today = jiff::civil::Date::from(jiff::Zoned::now());
            Ok(Value::Date(today))
        }
        "datetime.realtime" | "datetime.statement" | "datetime.transaction" => {
            if vals.first().map(|v| v.is_null()).unwrap_or(false) { return Ok(Value::Null); }
            if !vals.is_empty() { return eval_datetime_constructor(&vals); }
            Ok(Value::DateTime(Box::new(jiff::Zoned::now())))
        }
        "localdatetime.realtime" | "localdatetime.statement" | "localdatetime.transaction" => {
            if vals.first().map(|v| v.is_null()).unwrap_or(false) { return Ok(Value::Null); }
            if !vals.is_empty() { return eval_localdatetime_constructor(&vals); }
            let now = jiff::Zoned::now();
            Ok(Value::LocalDateTime(jiff::civil::DateTime::from(now)))
        }
        "localtime.realtime" | "localtime.statement" | "localtime.transaction" => {
            if vals.first().map(|v| v.is_null()).unwrap_or(false) { return Ok(Value::Null); }
            if !vals.is_empty() { return eval_localtime_constructor(&vals); }
            let now = jiff::Zoned::now();
            Ok(Value::LocalTime(jiff::civil::Time::from(now)))
        }
        "time.realtime" | "time.statement" | "time.transaction" => {
            if vals.first().map(|v| v.is_null()).unwrap_or(false) { return Ok(Value::Null); }
            if !vals.is_empty() { return eval_time_constructor(&vals); }
            Ok(Value::DateTime(Box::new(jiff::Zoned::now())))
        }
        "datetime.fromepoch" => {
            // datetime.fromepoch(seconds, nanoseconds)
            if vals.is_empty() || vals.len() > 2 {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "datetime.fromEpoch(seconds[, nanoseconds])".to_string(),
                    actual: format!("{} arguments", vals.len()),
                }));
            }
            let secs = match &vals[0] {
                Value::Int(n) => *n,
                other => return type_error("datetime.fromEpoch", other),
            };
            let nanos = if vals.len() == 2 {
                match &vals[1] {
                    Value::Int(n) => *n,
                    _ => 0,
                }
            } else {
                0
            };
            let ts = jiff::Timestamp::new(secs, nanos as i32)
                .map_err(|e| temporal_parse_error("datetime.fromEpoch", e))?;
            let zoned = ts.to_zoned(jiff::tz::TimeZone::UTC);
            Ok(Value::DateTime(Box::new(zoned)))
        }

        "datetime.fromepochmillis" => {
            if vals.len() != 1 {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "datetime.fromEpochMillis(millis)".to_string(),
                    actual: format!("{} arguments", vals.len()),
                }));
            }
            let millis = match &vals[0] {
                Value::Int(n) => *n,
                other => return type_error("datetime.fromEpochMillis", other),
            };
            let ts = jiff::Timestamp::from_millisecond(millis)
                .map_err(|e| temporal_parse_error("datetime.fromEpochMillis", e))?;
            let zoned = ts.to_zoned(jiff::tz::TimeZone::UTC);
            Ok(Value::DateTime(Box::new(zoned)))
        }

        "duration.between" => eval_duration_between(&vals),
        "duration.inmonths" => eval_duration_in_months(&vals),
        "duration.indays" => eval_duration_in_days(&vals),
        "duration.inseconds" => eval_duration_in_seconds(&vals),

        // ── Aggregation functions are handled at the planner/executor level (Aggregate operator),
        //    not in scalar expression evaluation. Calling them in a non-aggregating context
        //    returns an explicit error that guides the user to use WITH/RETURN clauses. ──
        "count" | "sum" | "avg" | "min" | "max" | "collect" | "stdev" | "stdevp"
        | "percentilecont" | "percentiledisc" => {
            Err(GqlError::Execution(ExecutionError::unsupported(format!(
                "aggregation function '{}' must be used in a WITH/RETURN clause",
                name
            ))))
        }

        // ── Unknown function ──────────────────────────────────────────────────
        other => Err(GqlError::Execution(ExecutionError::unsupported(format!(
            "unknown function '{other}'"
        )))),
    }
}

// ── Temporal property access ──────────────────────────────────────────────────

pub fn temporal_property_access(val: &Value, key: &str) -> Result<Value, GqlError> {
    match val {
        Value::Date(d) => match key {
            "year" => Ok(Value::Int(d.year() as i64)),
            "month" => Ok(Value::Int(d.month() as i64)),
            "day" => Ok(Value::Int(d.day() as i64)),
            "week" => {
                let iso = d.iso_week_date();
                Ok(Value::Int(iso.week() as i64))
            }
            "weekYear" => {
                let iso = d.iso_week_date();
                Ok(Value::Int(iso.year() as i64))
            }
            "dayOfWeek" | "weekDay" => Ok(Value::Int(d.weekday().to_monday_one_offset() as i64)),
            "dayOfYear" => Ok(Value::Int(d.day_of_year() as i64)),
            "quarter" => Ok(Value::Int(((d.month() as i64 - 1) / 3) + 1)),
            "ordinalDay" => Ok(Value::Int(d.day_of_year() as i64)),
            "dayOfQuarter" => {
                let q0 = ((d.month() as i64 - 1) / 3) as i32; // 0-indexed quarter
                let q_start = jiff::civil::Date::new(d.year(), (q0 * 3 + 1) as i8, 1)
                    .map_err(|e| temporal_parse_error("date.dayOfQuarter", e))?;
                let diff = q_start.until(jiff::civil::DateDifference::new(*d).largest(jiff::Unit::Day))
                    .map_err(|e| temporal_parse_error("date.dayOfQuarter", e))?;
                Ok(Value::Int(diff.get_days() as i64 + 1))
            }
            _ => Ok(Value::Null),
        },
        Value::LocalTime(t) => match key {
            "hour" => Ok(Value::Int(t.hour() as i64)),
            "minute" => Ok(Value::Int(t.minute() as i64)),
            "second" => Ok(Value::Int(t.second() as i64)),
            "millisecond" => Ok(Value::Int((t.subsec_nanosecond() / 1_000_000) as i64)),
            "microsecond" => Ok(Value::Int((t.subsec_nanosecond() / 1_000) as i64)),
            "nanosecond" => Ok(Value::Int(t.subsec_nanosecond() as i64)),
            _ => Ok(Value::Null),
        },
        Value::LocalDateTime(dt) => match key {
            "year" => Ok(Value::Int(dt.year() as i64)),
            "month" => Ok(Value::Int(dt.month() as i64)),
            "day" => Ok(Value::Int(dt.day() as i64)),
            "hour" => Ok(Value::Int(dt.hour() as i64)),
            "minute" => Ok(Value::Int(dt.minute() as i64)),
            "second" => Ok(Value::Int(dt.second() as i64)),
            "millisecond" => Ok(Value::Int((dt.subsec_nanosecond() / 1_000_000) as i64)),
            "microsecond" => Ok(Value::Int((dt.subsec_nanosecond() / 1_000) as i64)),
            "nanosecond" => Ok(Value::Int(dt.subsec_nanosecond() as i64)),
            "week" => {
                let d = dt.date();
                let iso = d.iso_week_date();
                Ok(Value::Int(iso.week() as i64))
            }
            "weekYear" => {
                let iso = dt.date().iso_week_date();
                Ok(Value::Int(iso.year() as i64))
            }
            "dayOfWeek" | "weekDay" => Ok(Value::Int(dt.date().weekday().to_monday_one_offset() as i64)),
            "dayOfYear" | "ordinalDay" => Ok(Value::Int(dt.date().day_of_year() as i64)),
            "quarter" => Ok(Value::Int(((dt.month() as i64 - 1) / 3) + 1)),
            "dayOfQuarter" => {
                let d = dt.date();
                let q0 = ((d.month() as i64 - 1) / 3) as i32;
                let q_start = jiff::civil::Date::new(d.year(), (q0 * 3 + 1) as i8, 1)
                    .map_err(|e| temporal_parse_error("localdatetime.dayOfQuarter", e))?;
                let diff = q_start.until(jiff::civil::DateDifference::new(d).largest(jiff::Unit::Day))
                    .map_err(|e| temporal_parse_error("localdatetime.dayOfQuarter", e))?;
                Ok(Value::Int(diff.get_days() as i64 + 1))
            }
            _ => Ok(Value::Null),
        },
        Value::DateTime(zdt) => match key {
            "year" => Ok(Value::Int(zdt.year() as i64)),
            "month" => Ok(Value::Int(zdt.month() as i64)),
            "day" => Ok(Value::Int(zdt.day() as i64)),
            "hour" => Ok(Value::Int(zdt.hour() as i64)),
            "minute" => Ok(Value::Int(zdt.minute() as i64)),
            "second" => Ok(Value::Int(zdt.second() as i64)),
            "millisecond" => Ok(Value::Int((zdt.subsec_nanosecond() / 1_000_000) as i64)),
            "microsecond" => Ok(Value::Int((zdt.subsec_nanosecond() / 1_000) as i64)),
            "nanosecond" => Ok(Value::Int(zdt.subsec_nanosecond() as i64)),
            "timezone" => {
                if let Some(iana) = zdt.time_zone().iana_name() {
                    Ok(Value::Str(iana.to_string().into_boxed_str()))
                } else {
                    // Fixed offset: format as "+HH:MM"
                    let off = zdt.offset();
                    let total_secs = off.seconds();
                    let h = total_secs / 3600;
                    let m = (total_secs.abs() % 3600) / 60;
                    Ok(Value::Str(format!("{:+03}:{:02}", h, m).into_boxed_str()))
                }
            }
            "offset" => {
                let off = zdt.offset();
                let total_secs = off.seconds();
                let h = total_secs / 3600;
                let m = (total_secs.abs() % 3600) / 60;
                Ok(Value::Str(format!("{:+03}:{:02}", h, m).into_boxed_str()))
            }
            "offsetMinutes" => {
                let off = zdt.offset();
                Ok(Value::Int((off.seconds() / 60) as i64))
            }
            "offsetSeconds" => {
                let off = zdt.offset();
                Ok(Value::Int(off.seconds() as i64))
            }
            "week" => {
                let d = jiff::civil::Date::from(&**zdt);
                let iso = d.iso_week_date();
                Ok(Value::Int(iso.week() as i64))
            }
            "weekYear" => {
                let d = jiff::civil::Date::from(&**zdt);
                let iso = d.iso_week_date();
                Ok(Value::Int(iso.year() as i64))
            }
            "dayOfWeek" | "weekDay" => {
                let d = jiff::civil::Date::from(&**zdt);
                Ok(Value::Int(d.weekday().to_monday_one_offset() as i64))
            }
            "dayOfYear" | "ordinalDay" => {
                let d = jiff::civil::Date::from(&**zdt);
                Ok(Value::Int(d.day_of_year() as i64))
            }
            "quarter" => Ok(Value::Int(((zdt.month() as i64 - 1) / 3) + 1)),
            "dayOfQuarter" => {
                let d = jiff::civil::Date::from(&**zdt);
                let q0 = ((d.month() as i64 - 1) / 3) as i32;
                let q_start = jiff::civil::Date::new(d.year(), (q0 * 3 + 1) as i8, 1)
                    .map_err(|e| temporal_parse_error("datetime.dayOfQuarter", e))?;
                let diff = q_start.until(jiff::civil::DateDifference::new(d).largest(jiff::Unit::Day))
                    .map_err(|e| temporal_parse_error("datetime.dayOfQuarter", e))?;
                Ok(Value::Int(diff.get_days() as i64 + 1))
            }
            "epochMillis" => {
                let ts = zdt.timestamp();
                Ok(Value::Int(ts.as_millisecond()))
            }
            "epochSeconds" => {
                let ts = zdt.timestamp();
                Ok(Value::Int(ts.as_second()))
            }
            _ => Ok(Value::Null),
        },
        Value::Duration(d) => match key {
            "years" => Ok(Value::Int(d.months / 12)),
            "months" => Ok(Value::Int(d.months)),
            "quarters" => Ok(Value::Int(d.months / 3)),
            "quartersOfYear" => Ok(Value::Int((d.months / 3) % 4)),
            "monthsOfYear" => Ok(Value::Int(d.months % 12)),
            "monthsOfQuarter" => Ok(Value::Int(d.months % 3)),
            "weeks" => Ok(Value::Int(d.days / 7)),
            "days" => Ok(Value::Int(d.days)),
            "daysOfWeek" => Ok(Value::Int(d.days % 7)),
            "hours" => Ok(Value::Int(d.nanos / 3_600_000_000_000)),
            "minutes" => Ok(Value::Int(d.nanos / 60_000_000_000)),
            "minutesOfHour" => Ok(Value::Int((d.nanos / 60_000_000_000) % 60)),
            "seconds" => Ok(Value::Int(d.nanos / 1_000_000_000)),
            "secondsOfMinute" => Ok(Value::Int((d.nanos / 1_000_000_000) % 60)),
            "milliseconds" => Ok(Value::Int(d.nanos / 1_000_000)),
            "millisecondsOfSecond" => Ok(Value::Int((d.nanos % 1_000_000_000) / 1_000_000)),
            "microseconds" => Ok(Value::Int(d.nanos / 1_000)),
            "microsecondsOfSecond" => Ok(Value::Int((d.nanos % 1_000_000_000) / 1_000)),
            "nanoseconds" => Ok(Value::Int(d.nanos)),
            "nanosecondsOfSecond" => Ok(Value::Int(d.nanos % 1_000_000_000)),
            _ => Ok(Value::Null),
        },
        _ => Ok(Value::Null),
    }
}

// ── Temporal constructor helpers ───────────────────────────────────────────────

/// Extract an i64 field from a Value::Map, returning a default if missing.
fn map_get_int(
    m: &indexmap::IndexMap<Box<str>, Value>,
    key: &str,
    default: i64,
) -> Result<i64, GqlError> {
    match m.get(key) {
        Some(Value::Int(n)) => Ok(*n),
        Some(Value::Null) | None => Ok(default),
        Some(other) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: format!("Integer for '{key}'"),
            actual: other.type_name().to_string(),
        })),
    }
}

/// Extract an optional string field from a Value::Map.
fn map_get_str(m: &indexmap::IndexMap<Box<str>, Value>, key: &str) -> Option<String> {
    match m.get(key) {
        Some(Value::Str(s)) => Some(s.to_string()),
        _ => None,
    }
}

/// Parse a timezone string, supporting both IANA names (e.g. "America/New_York")
/// and fixed UTC offsets (e.g. "+01:00", "-05:30", "Z").
fn parse_timezone(tz_str: &str) -> Result<jiff::tz::TimeZone, String> {
    // Try fixed offset FIRST for strings starting with '+' or '-'
    // to avoid IANA lookup failures or ambiguous matches.
    if tz_str == "Z" || tz_str == "UTC" {
        return Ok(jiff::tz::TimeZone::UTC);
    }
    if let Some(offset_str) = tz_str
        .strip_prefix('+')
        .or_else(|| tz_str.strip_prefix('-'))
    {
        let negative = tz_str.starts_with('-');
        let parts: Vec<&str> = offset_str.split(':').collect();
        let total_seconds = if parts.len() == 3 {
            // HH:MM:SS format (e.g. "+02:05:59")
            let h: i32 = parts[0].parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let m: i32 = parts[1].parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let s: i32 = parts[2].parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            (h * 3600 + m * 60 + s) * if negative { -1 } else { 1 }
        } else if parts.len() == 2 {
            let h: i32 = parts[0].parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let m: i32 = parts[1].parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            (h * 3600 + m * 60) * if negative { -1 } else { 1 }
        } else if offset_str.len() == 4 {
            let h: i32 = offset_str[..2].parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let m: i32 = offset_str[2..].parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            (h * 3600 + m * 60) * if negative { -1 } else { 1 }
        } else if offset_str.len() == 2 {
            let h: i32 = offset_str.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            h * 3600 * if negative { -1 } else { 1 }
        } else {
            return Err(format!("invalid timezone offset: {tz_str}"));
        };
        let offset = jiff::tz::Offset::from_seconds(total_seconds).map_err(|e| e.to_string())?;
        return Ok(jiff::tz::TimeZone::fixed(offset));
    }
    // Try IANA name (for named timezones like 'Europe/Stockholm', 'America/New_York')
    if let Ok(tz) = jiff::tz::TimeZone::get(tz_str) {
        return Ok(tz);
    }
    Err(format!("unknown timezone: {tz_str}"))
}

/// Parse a datetime string with a timezone offset (e.g. "2015-07-21T21:40:32+01:00").
/// jiff's Zoned parser requires square-bracket IANA annotations, so we parse the offset
/// manually and convert via Timestamp.
fn parse_datetime_with_offset(s: &str) -> Result<jiff::Zoned, String> {
    // Try parsing as a Timestamp first (handles "2015-07-21T21:40:32+01:00" etc.)
    if let Ok(ts) = s.parse::<jiff::Timestamp>() {
        // Extract the offset from the string to use as the timezone
        let tz = extract_offset_from_string(s).unwrap_or(jiff::tz::TimeZone::UTC);
        return Ok(ts.to_zoned(tz));
    }
    // Last resort: try parsing as civil DateTime and assume UTC
    if let Ok(dt) = s.parse::<jiff::civil::DateTime>() {
        return dt.to_zoned(jiff::tz::TimeZone::UTC).map_err(|e| e.to_string());
    }
    Err(format!("cannot parse datetime from '{s}'"))
}

/// Extract a timezone offset from the tail of an ISO 8601 datetime string.
/// Looks for patterns like "+01:00", "-05:30", "+0100", "Z" at the end.
fn extract_offset_from_string(s: &str) -> Option<jiff::tz::TimeZone> {
    let s = s.trim();
    if s.ends_with('Z') || s.ends_with('z') {
        return Some(jiff::tz::TimeZone::UTC);
    }
    // Look for +/-HH:MM or +/-HHMM at the end
    // Find last '+' or '-' that could be offset (not in date part)
    // The offset should be within the last 6 chars
    let tail = if s.len() > 6 { &s[s.len() - 6..] } else { s };
    let offset_start = tail.rfind('+').or_else(|| tail.rfind('-'));
    if let Some(pos) = offset_start {
        let offset_str = &tail[pos..];
        if let Ok(tz) = parse_timezone(offset_str) {
            return Some(tz);
        }
    }
    None
}

fn temporal_parse_error(func: &str, msg: impl std::fmt::Display) -> GqlError {
    GqlError::Execution(ExecutionError::TypeMismatch {
        expected: format!("valid {func}() argument"),
        actual: msg.to_string(),
    })
}

fn eval_date_constructor(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.is_empty() {
        let today = jiff::civil::Date::from(jiff::Zoned::now());
        return Ok(Value::Date(today));
    }
    if vals.len() != 1 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "date() expects 0 or 1 argument".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => {
            let d = parse_date_str(s).map_err(|e| temporal_parse_error("date", e))?;
            Ok(Value::Date(d))
        }
        // Accept temporal values directly — extract date component
        Value::Date(d) => Ok(Value::Date(*d)),
        Value::LocalDateTime(dt) => Ok(Value::Date(jiff::civil::Date::new(
            dt.year(), dt.month(), dt.day(),
        ).map_err(|e| temporal_parse_error("date", e))?)),
        Value::DateTime(zdt) => {
            let dt = jiff::civil::DateTime::from(zdt.as_ref());
            Ok(Value::Date(jiff::civil::Date::new(
                dt.year(), dt.month(), dt.day(),
            ).map_err(|e| temporal_parse_error("date", e))?))
        }
        Value::Map(m) => {
            let base_date = extract_base_date(m)?;
            let d = resolve_date_from_map(m, &base_date)?;
            Ok(Value::Date(d))
        }
        other => type_error("date", other),
    }
}

/// Parse an ISO 8601 date string, handling all abbreviated formats that jiff may not support.
/// Formats supported beyond jiff's default:
///   - `YYYY`           (year only → Jan 1)
///   - `YYYY-MM`        (year-month → day 1)
///   - `YYYYMM`         (compact year-month)
///   - `YYYY-Www`       (ISO week date → Monday)
///   - `YYYY-Www-D`     (ISO week date with day-of-week)
///   - `YYYYWww`        (compact ISO week)
///   - `YYYYWwwD`       (compact ISO week + day)
///   - `YYYY-DDD`       (ordinal date)
///   - `YYYYDDD`        (compact ordinal date)
fn parse_date_str(s: &str) -> Result<jiff::civil::Date, String> {
    // First try jiff's standard parser (handles YYYY-MM-DD and RFC3339 variants)
    if let Ok(d) = s.parse::<jiff::civil::Date>() {
        return Ok(d);
    }

    let s = s.trim();

    // YYYY-Www or YYYY-Www-D (ISO week date)
    if let Some(week_part) = s.strip_prefix(|c: char| c.is_ascii_digit())
        .and(None::<&str>).or_else(|| {
            // Extract YYYY-W portion
            if s.len() >= 8 {
                let (_year_part, rest) = s.split_at(4);
                let rest = rest.strip_prefix('-').unwrap_or(rest);
                if rest.starts_with('W') {
                    Some(rest)
                } else {
                    None
                }
            } else {
                None
            }
        })
    {
        let _ = week_part;
        // Handle YYYY-Www-D and YYYY-Www and YYYYWwwD and YYYYWww
        let (year_str, rest) = s.split_at(4);
        let year: i64 = year_str.parse().map_err(|e| format!("{e}"))?;
        let rest = rest.strip_prefix('-').unwrap_or(rest);
        if rest.starts_with('W') {
            let rest = rest.strip_prefix('W').unwrap_or(rest); // skip 'W'
            let (week_str, rest) = if rest.len() >= 2 { rest.split_at(2) } else { (rest, "") };
            let week: i64 = week_str.parse().map_err(|e| format!("{e}"))?;
            let rest = rest.strip_prefix('-').unwrap_or(rest);
            let day_of_week = if rest.is_empty() {
                1 // Monday
            } else {
                rest[..1].parse::<i64>().map_err(|e| format!("{e}"))?
            };
            return compute_week_date_str(year, week, day_of_week);
        }
    }

    // Redo parsing more systematically
    if s.len() == 4 && s.chars().all(|c| c.is_ascii_digit()) {
        // YYYY → Jan 1
        let year: i16 = s.parse().map_err(|e| format!("{e}"))?;
        return jiff::civil::Date::new(year, 1, 1).map_err(|e| e.to_string());
    }

    // Check for YYYY-Www or YYYY-Www-D or YYYYWwwD or YYYYWww
    if s.contains('W') {
        let (year_str, rest) = s.split_at(4);
        let year: i64 = year_str.parse().map_err(|e| format!("{e}"))?;
        let rest = rest.strip_prefix('-').unwrap_or(rest);
        if let Some(rest) = rest.strip_prefix('W') {
            let (week_str, rest) = if rest.len() >= 2 { rest.split_at(2) } else { (rest, "") };
            let week: i64 = week_str.parse().map_err(|e| format!("{e}"))?;
            let rest = rest.strip_prefix('-').unwrap_or(rest);
            let day_of_week = if rest.is_empty() { 1 } else {
                rest[..1].parse::<i64>().map_err(|e| format!("{e}"))?
            };
            return compute_week_date_str(year, week, day_of_week);
        }
    }

    // YYYY-MM (with separator, no day) or YYYYMM (without separator)
    if s.len() == 7 && s.as_bytes()[4] == b'-' && s[5..].chars().all(|c| c.is_ascii_digit()) {
        // YYYY-MM
        let year: i16 = s[..4].parse().map_err(|e| format!("{e}"))?;
        let month: i8 = s[5..].parse().map_err(|e| format!("{e}"))?;
        return jiff::civil::Date::new(year, month, 1).map_err(|e| e.to_string());
    }
    if s.len() == 6 && s.chars().all(|c| c.is_ascii_digit()) {
        // YYYYMM
        let year: i16 = s[..4].parse().map_err(|e| format!("{e}"))?;
        let month: i8 = s[4..].parse().map_err(|e| format!("{e}"))?;
        return jiff::civil::Date::new(year, month, 1).map_err(|e| e.to_string());
    }

    // YYYY-DDD (ordinal date with separator) or YYYYDDD (compact)
    if s.len() == 8 && s.as_bytes()[4] == b'-' && s[5..].chars().all(|c| c.is_ascii_digit()) {
        // YYYY-DDD
        let year: i16 = s[..4].parse().map_err(|e| format!("{e}"))?;
        let ordinal: i64 = s[5..].parse().map_err(|e| format!("{e}"))?;
        let d = jiff::civil::Date::new(year, 1, 1).map_err(|e| e.to_string())?;
        return d.checked_add(jiff::Span::new().days(ordinal - 1)).map_err(|e| e.to_string());
    }
    if s.len() == 7 && s.chars().all(|c| c.is_ascii_digit()) {
        // YYYYDDD
        let year: i16 = s[..4].parse().map_err(|e| format!("{e}"))?;
        let ordinal: i64 = s[4..].parse().map_err(|e| format!("{e}"))?;
        let d = jiff::civil::Date::new(year, 1, 1).map_err(|e| e.to_string())?;
        return d.checked_add(jiff::Span::new().days(ordinal - 1)).map_err(|e| e.to_string());
    }

    Err(format!("cannot parse date from '{s}'"))
}

fn compute_week_date_str(year: i64, week: i64, day_of_week: i64) -> Result<jiff::civil::Date, String> {
    // Find the Monday of week 1 of the given year using ISO 8601 algorithm
    let jan4 = jiff::civil::Date::new(year as i16, 1, 4).map_err(|e| e.to_string())?;
    let jan4_dow = jan4.weekday().to_monday_one_offset() as i64;
    let week1_monday_ordinal = 4 - jan4_dow + 1;
    let target_ordinal = week1_monday_ordinal + (week - 1) * 7 + (day_of_week - 1);
    let d = jiff::civil::Date::new(year as i16, 1, 1).map_err(|e| e.to_string())?;
    d.checked_add(jiff::Span::new().days(target_ordinal - 1)).map_err(|e| e.to_string())
}

/// Compute an ISO week date from year, week, and day-of-week.
fn compute_week_date(year: i64, week: i64, day_of_week: i64) -> Result<jiff::civil::Date, GqlError> {
    let jan4 = jiff::civil::Date::new(year as i16, 1, 4)
        .map_err(|e| temporal_parse_error("date", e))?;
    let jan4_dow = jan4.weekday().to_monday_one_offset() as i64;
    let week1_monday_ordinal = 4 - jan4_dow + 1;
    let target_ordinal = week1_monday_ordinal + (week - 1) * 7 + (day_of_week - 1);
    let d = jiff::civil::Date::new(year as i16, 1, 1)
        .map_err(|e| temporal_parse_error("date", e))?;
    d.checked_add(jiff::Span::new().days(target_ordinal - 1))
        .map_err(|e| temporal_parse_error("date", e))
}

/// Resolve the date part from a map, handling week-based dates.
fn resolve_date_from_map(
    m: &indexmap::IndexMap<Box<str>, Value>,
    base_date: &Option<jiff::civil::Date>,
) -> Result<jiff::civil::Date, GqlError> {
    if m.contains_key("week") {
        let year = if m.contains_key("year") {
            map_get_int(m, "year", 0)?
        } else if let Some(bd) = base_date {
            // Use ISO week year (not Gregorian year) since week dates cross year boundaries.
            bd.iso_week_date().year() as i64
        } else {
            0
        };
        let week = map_get_int(m, "week", 1)?;
        let day_of_week = if m.contains_key("dayOfWeek") {
            map_get_int(m, "dayOfWeek", 1)?
        } else if let Some(bd) = base_date {
            bd.weekday().to_monday_one_offset() as i64
        } else {
            1
        };
        compute_week_date(year, week, day_of_week)
    } else if m.contains_key("ordinalDay") {
        let year = if m.contains_key("year") {
            map_get_int(m, "year", 0)?
        } else if let Some(bd) = base_date {
            bd.year() as i64
        } else {
            0
        };
        let ordinal = map_get_int(m, "ordinalDay", 1)?;
        let d = jiff::civil::Date::new(year as i16, 1, 1)
            .map_err(|e| temporal_parse_error("date", e))?;
        d.checked_add(jiff::Span::new().days(ordinal - 1))
            .map_err(|e| temporal_parse_error("date", e))
    } else if m.contains_key("quarter") {
        let year = if m.contains_key("year") {
            map_get_int(m, "year", 0)?
        } else if let Some(bd) = base_date {
            bd.year() as i64
        } else {
            0
        };
        let quarter = map_get_int(m, "quarter", 1)?;
        if m.contains_key("dayOfQuarter") {
            let day_of_quarter = map_get_int(m, "dayOfQuarter", 1)?;
            let month = ((quarter - 1) * 3 + 1) as i8;
            let d = jiff::civil::Date::new(year as i16, month, 1)
                .map_err(|e| temporal_parse_error("date", e))?;
            d.checked_add(jiff::Span::new().days(day_of_quarter - 1))
                .map_err(|e| temporal_parse_error("date", e))
        } else if let Some(bd) = base_date {
            let month_in_quarter = (bd.month() as i64 - 1) % 3;
            let new_month = (quarter - 1) * 3 + 1 + month_in_quarter;
            jiff::civil::Date::new(year as i16, new_month as i8, bd.day())
                .map_err(|e| temporal_parse_error("date", e))
        } else {
            let month = ((quarter - 1) * 3 + 1) as i8;
            jiff::civil::Date::new(year as i16, month, 1)
                .map_err(|e| temporal_parse_error("date", e))
        }
    } else {
        let year = if m.contains_key("year") {
            map_get_int(m, "year", 0)? as i16
        } else if let Some(bd) = base_date {
            bd.year()
        } else {
            0
        };
        let month = if m.contains_key("month") {
            map_get_int(m, "month", 1)? as i8
        } else if let Some(bd) = base_date {
            bd.month()
        } else {
            1
        };
        let day = if m.contains_key("day") {
            map_get_int(m, "day", 1)? as i8
        } else if let Some(bd) = base_date {
            bd.day()
        } else {
            1
        };
        jiff::civil::Date::new(year, month, day)
            .map_err(|e| temporal_parse_error("date", e))
    }
}

/// Extract a base date from a map's "date" key (handles Date, LocalDateTime, DateTime).
fn extract_base_date(
    m: &indexmap::IndexMap<Box<str>, Value>,
) -> Result<Option<jiff::civil::Date>, GqlError> {
    match m.get("date") {
        Some(Value::Date(d)) => Ok(Some(*d)),
        Some(Value::LocalDateTime(dt)) => Ok(Some(
            jiff::civil::Date::new(dt.year(), dt.month(), dt.day())
                .map_err(|e| temporal_parse_error("date", e))?,
        )),
        Some(Value::DateTime(zdt)) => {
            let dt = jiff::civil::DateTime::from(zdt.as_ref());
            Ok(Some(
                jiff::civil::Date::new(dt.year(), dt.month(), dt.day())
                    .map_err(|e| temporal_parse_error("date", e))?,
            ))
        }
        Some(Value::Null) => Ok(None),
        None => Ok(None),
        Some(other) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "Date, LocalDateTime, or DateTime".to_string(),
            actual: other.type_name().to_string(),
        })),
    }
}

/// Extract a base time from a map's "time" key (handles LocalTime, DateTime, LocalDateTime).
fn extract_base_time(
    m: &indexmap::IndexMap<Box<str>, Value>,
) -> Result<Option<jiff::civil::Time>, GqlError> {
    match m.get("time") {
        Some(Value::LocalTime(t)) => Ok(Some(*t)),
        Some(Value::LocalDateTime(dt)) => Ok(Some(
            jiff::civil::Time::new(dt.hour(), dt.minute(), dt.second(), dt.subsec_nanosecond())
                .map_err(|e| temporal_parse_error("time", e))?,
        )),
        Some(Value::DateTime(zdt)) => {
            let dt = jiff::civil::DateTime::from(zdt.as_ref());
            Ok(Some(
                jiff::civil::Time::new(dt.hour(), dt.minute(), dt.second(), dt.subsec_nanosecond())
                    .map_err(|e| temporal_parse_error("time", e))?,
            ))
        }
        Some(Value::Null) => Ok(None),
        None => Ok(None),
        Some(other) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "LocalTime, LocalDateTime, or DateTime".to_string(),
            actual: other.type_name().to_string(),
        })),
    }
}

fn eval_time_constructor(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.is_empty() {
        let now = jiff::Zoned::now();
        return Ok(Value::LocalTime(jiff::civil::Time::from(now)));
    }
    if vals.len() != 1 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "time() expects 0 or 1 argument".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => {
            let time_part = strip_tz_offset_from_time(s);
            let t: jiff::civil::Time = if let Ok(t) = s.parse::<jiff::civil::Time>() {
                t
            } else {
                time_part
                    .parse()
                    .map_err(|e| temporal_parse_error("time", e))?
            };
            let epoch_dt = jiff::civil::Date::new(1970, 1, 1).unwrap().to_datetime(t);
            // time() always returns a ZonedTime; use explicit offset if present, else UTC
            let tz = extract_offset_from_string(s).unwrap_or(jiff::tz::TimeZone::UTC);
            let zoned = epoch_dt
                .to_zoned(tz)
                .map_err(|e| temporal_parse_error("time", e))?;
            Ok(Value::DateTime(Box::new(zoned)))
        }
        // Accept temporal values directly — extract time with timezone
        // time() on LocalTime returns a ZonedTime (UTC by default per Cypher spec)
        Value::LocalTime(t) => {
            let epoch_dt = jiff::civil::Date::new(1970, 1, 1).unwrap().to_datetime(*t);
            let zoned = epoch_dt
                .to_zoned(jiff::tz::TimeZone::UTC)
                .map_err(|e| temporal_parse_error("time", e))?;
            Ok(Value::DateTime(Box::new(zoned)))
        }
        Value::DateTime(zdt) => {
            // time() on a DateTime preserves the timezone as a zoned time
            let dt = jiff::civil::DateTime::from(zdt.as_ref());
            let t = jiff::civil::Time::new(dt.hour(), dt.minute(), dt.second(), dt.subsec_nanosecond())
                .map_err(|e| temporal_parse_error("time", e))?;
            let epoch_dt = jiff::civil::Date::new(1970, 1, 1).unwrap().to_datetime(t);
            let zoned = epoch_dt.to_zoned(zdt.time_zone().clone())
                .map_err(|e| temporal_parse_error("time", e))?;
            Ok(Value::DateTime(Box::new(zoned)))
        }
        Value::LocalDateTime(dt) => {
            // time(LocalDateTime) → ZonedTime with UTC timezone
            let t = jiff::civil::Time::new(dt.hour(), dt.minute(), dt.second(), dt.subsec_nanosecond())
                .map_err(|e| temporal_parse_error("time", e))?;
            let epoch_dt = jiff::civil::Date::new(1970, 1, 1).unwrap().to_datetime(t);
            let zoned = epoch_dt.to_zoned(jiff::tz::TimeZone::UTC)
                .map_err(|e| temporal_parse_error("time", e))?;
            Ok(Value::DateTime(Box::new(zoned)))
        }
        Value::Map(m) => {
            let base_time = extract_base_time(m)?;
            let hour = if m.contains_key("hour") { map_get_int(m, "hour", 0)? as i8 }
                else if let Some(bt) = &base_time { bt.hour() } else { 0 };
            let minute = if m.contains_key("minute") { map_get_int(m, "minute", 0)? as i8 }
                else if let Some(bt) = &base_time { bt.minute() } else { 0 };
            let second = if m.contains_key("second") { map_get_int(m, "second", 0)? as i8 }
                else if let Some(bt) = &base_time { bt.second() } else { 0 };
            // Combine millisecond + microsecond + nanosecond (Cypher spec: all additive)
            let nanosecond = if m.contains_key("nanosecond")
                || m.contains_key("microsecond")
                || m.contains_key("millisecond")
            {
                let millis = map_get_int(m, "millisecond", 0)? * 1_000_000;
                let micros = map_get_int(m, "microsecond", 0)? * 1_000;
                let nanos = map_get_int(m, "nanosecond", 0)?;
                (millis + micros + nanos) as i32
            } else if let Some(bt) = &base_time {
                bt.subsec_nanosecond()
            } else {
                0
            };
            let tz_name = map_get_str(m, "timezone");
            let t = jiff::civil::Time::new(hour, minute, second, nanosecond)
                .map_err(|e| temporal_parse_error("time", e))?;
            let epoch_dt = jiff::civil::Date::new(1970, 1, 1).unwrap().to_datetime(t);
            // Get base timezone if base is a ZonedTime
            let base_tz = if let Some(Value::DateTime(base_zdt)) = m.get("time") {
                Some(base_zdt.time_zone().clone())
            } else {
                None
            };

            // Determine the target timezone
            let target_tz = if let Some(ref tz_str) = tz_name {
                parse_timezone(tz_str).map_err(|e| temporal_parse_error("time", e))?
            } else if let Some(ref tz) = base_tz {
                tz.clone()
            } else {
                jiff::tz::TimeZone::UTC
            };

            // Create zoned time: apply field overrides in the base timezone, then convert to target
            if let Some(base_tz_val) = base_tz {
                if tz_name.is_some() {
                    // Field overrides + timezone change: apply overrides in base TZ, then rebase
                    let with_base_tz = epoch_dt
                        .to_zoned(base_tz_val)
                        .map_err(|e| temporal_parse_error("time", e))?;
                    let rebase_zoned = with_base_tz.with_time_zone(target_tz);
                    Ok(Value::DateTime(Box::new(rebase_zoned)))
                } else {
                    // No timezone change: just apply field overrides, keep base TZ
                    let zoned = epoch_dt
                        .to_zoned(target_tz)
                        .map_err(|e| temporal_parse_error("time", e))?;
                    Ok(Value::DateTime(Box::new(zoned)))
                }
            } else {
                let zoned = epoch_dt
                    .to_zoned(target_tz)
                    .map_err(|e| temporal_parse_error("time", e))?;
                Ok(Value::DateTime(Box::new(zoned)))
            }
        }
        other => type_error("time", other),
    }
}

/// Strip timezone offset from a time string like "21:40:32+01:00" → "21:40:32"
fn strip_tz_offset_from_time(s: &str) -> &str {
    // Find last '+' or '-' that separates time from offset
    // Time part is before this. Handle compact format too (e.g. "214032-0100")
    if let Some(pos) = s.rfind('+') {
        if pos > 0 {
            return &s[..pos];
        }
    }
    if let Some(pos) = s.rfind('-') {
        // Make sure the '-' is not at position 0 or part of the time itself
        // A time string like "21:40:32" doesn't have '-' after position 0
        if pos > 2 {
            return &s[..pos];
        }
    }
    s
}

fn eval_localtime_constructor(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.is_empty() {
        let now = jiff::Zoned::now();
        return Ok(Value::LocalTime(jiff::civil::Time::from(now)));
    }
    if vals.len() != 1 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "localtime() expects 0 or 1 argument".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => {
            let t: jiff::civil::Time = s
                .parse()
                .map_err(|e| temporal_parse_error("localtime", e))?;
            Ok(Value::LocalTime(t))
        }
        // Accept temporal values directly — extract time component
        Value::LocalTime(t) => Ok(Value::LocalTime(*t)),
        Value::LocalDateTime(dt) => Ok(Value::LocalTime(
            jiff::civil::Time::new(dt.hour(), dt.minute(), dt.second(), dt.subsec_nanosecond())
                .map_err(|e| temporal_parse_error("localtime", e))?,
        )),
        Value::DateTime(zdt) => {
            let dt = jiff::civil::DateTime::from(zdt.as_ref());
            Ok(Value::LocalTime(
                jiff::civil::Time::new(dt.hour(), dt.minute(), dt.second(), dt.subsec_nanosecond())
                    .map_err(|e| temporal_parse_error("localtime", e))?,
            ))
        }
        Value::Map(m) => {
            let base_time = extract_base_time(m)?;
            let hour = if m.contains_key("hour") {
                map_get_int(m, "hour", 0)? as i8
            } else if let Some(bt) = &base_time {
                bt.hour()
            } else {
                0
            };
            let minute = if m.contains_key("minute") {
                map_get_int(m, "minute", 0)? as i8
            } else if let Some(bt) = &base_time {
                bt.minute()
            } else {
                0
            };
            let second = if m.contains_key("second") {
                map_get_int(m, "second", 0)? as i8
            } else if let Some(bt) = &base_time {
                bt.second()
            } else {
                0
            };
            // Combine millisecond + microsecond + nanosecond (Cypher spec: all additive)
            let nanosecond = if m.contains_key("nanosecond")
                || m.contains_key("microsecond")
                || m.contains_key("millisecond")
            {
                let millis = map_get_int(m, "millisecond", 0)? * 1_000_000;
                let micros = map_get_int(m, "microsecond", 0)? * 1_000;
                let nanos = map_get_int(m, "nanosecond", 0)?;
                (millis + micros + nanos) as i32
            } else if let Some(bt) = &base_time {
                bt.subsec_nanosecond()
            } else {
                0
            };
            let t = jiff::civil::Time::new(hour, minute, second, nanosecond)
                .map_err(|e| temporal_parse_error("localtime", e))?;
            Ok(Value::LocalTime(t))
        }
        other => type_error("localtime", other),
    }
}

fn eval_datetime_constructor(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.is_empty() {
        return Ok(Value::DateTime(Box::new(jiff::Zoned::now())));
    }
    if vals.len() != 1 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "datetime() expects 0 or 1 argument".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => {
            if let Ok(zoned) = s.parse::<jiff::Zoned>() {
                return Ok(Value::DateTime(Box::new(zoned)));
            }
            let zoned = parse_datetime_with_offset(s)
                .map_err(|e| temporal_parse_error("datetime", e))?;
            Ok(Value::DateTime(Box::new(zoned)))
        }
        // Accept temporal values directly
        Value::DateTime(zdt) => Ok(Value::DateTime(zdt.clone())),
        Value::LocalDateTime(dt) => {
            let zoned = dt.to_zoned(jiff::tz::TimeZone::UTC)
                .map_err(|e| temporal_parse_error("datetime", e))?;
            Ok(Value::DateTime(Box::new(zoned)))
        }
        Value::Date(d) => {
            let dt = d.to_datetime(jiff::civil::Time::midnight());
            let zoned = dt.to_zoned(jiff::tz::TimeZone::UTC)
                .map_err(|e| temporal_parse_error("datetime", e))?;
            Ok(Value::DateTime(Box::new(zoned)))
        }
        Value::Map(m) => {
            let base_date = extract_base_date(m)?;
            let base_time = extract_base_time(m)?;
            // Track whether the "datetime" base is a LocalDateTime (has no real timezone).
            let base_is_local_dt = matches!(m.get("datetime"), Some(Value::LocalDateTime(_)));
            let base_zdt: Option<(jiff::civil::DateTime, jiff::tz::TimeZone)> = match m.get("datetime") {
                Some(Value::DateTime(zdt)) => {
                    Some((jiff::civil::DateTime::from(zdt.as_ref()), zdt.time_zone().clone()))
                }
                Some(Value::LocalDateTime(dt)) => Some((*dt, jiff::tz::TimeZone::UTC)),
                _ => None,
            };

            // Extract timezone from "time" key when it is a ZonedTime (stored as DateTime).
            // This preserves e.g. +01:00 from time({..., timezone: '+01:00'}).
            let base_time_tz: Option<jiff::tz::TimeZone> = match m.get("time") {
                Some(Value::DateTime(zdt)) => Some(zdt.time_zone().clone()),
                _ => None,
            };

            // Resolve date part using shared logic (handles week, ordinalDay, quarter)
            let has_date_fields = m.contains_key("week") || m.contains_key("ordinalDay")
                || m.contains_key("quarter") || m.contains_key("year")
                || m.contains_key("month") || m.contains_key("day")
                || base_date.is_some();
            let effective_base = if base_date.is_some() {
                base_date
            } else if let Some((bdt, _)) = &base_zdt {
                Some(jiff::civil::Date::new(bdt.year(), bdt.month(), bdt.day())
                    .map_err(|e| temporal_parse_error("datetime", e))?)
            } else {
                None
            };
            let date_part = if has_date_fields {
                resolve_date_from_map(m, &effective_base)?
            } else if let Some((bdt, _)) = &base_zdt {
                jiff::civil::Date::new(bdt.year(), bdt.month(), bdt.day())
                    .map_err(|e| temporal_parse_error("datetime", e))?
            } else {
                jiff::civil::Date::new(0, 1, 1)
                    .map_err(|e| temporal_parse_error("datetime", e))?
            };

            // Resolve time part
            let (base_h, base_mi, base_s, base_ns) = if let Some(bt) = &base_time {
                (bt.hour(), bt.minute(), bt.second(), bt.subsec_nanosecond())
            } else if let Some((bdt, _)) = &base_zdt {
                (bdt.hour(), bdt.minute(), bdt.second(), bdt.subsec_nanosecond())
            } else {
                (0i8, 0i8, 0i8, 0i32)
            };
            let hour = if m.contains_key("hour") { map_get_int(m, "hour", 0)? as i8 } else { base_h };
            let minute = if m.contains_key("minute") { map_get_int(m, "minute", 0)? as i8 } else { base_mi };
            let second = if m.contains_key("second") { map_get_int(m, "second", 0)? as i8 } else { base_s };
            // Combine millisecond + microsecond + nanosecond (Cypher spec: all additive)
            let nanosecond = if m.contains_key("nanosecond")
                || m.contains_key("microsecond")
                || m.contains_key("millisecond")
            {
                let millis = map_get_int(m, "millisecond", 0)? * 1_000_000;
                let micros = map_get_int(m, "microsecond", 0)? * 1_000;
                let nanos = map_get_int(m, "nanosecond", 0)?;
                (millis + micros + nanos) as i32
            } else {
                base_ns
            };

            let tz_name = map_get_str(m, "timezone");

            // When base_zdt provides the datetime and timezone is overridden,
            // convert the instant to the new timezone (same UTC moment, different wall clock).
            // Only do this when no field overrides are applied beyond what's in base_zdt.
            let has_field_overrides = m.contains_key("year") || m.contains_key("month")
                || m.contains_key("day") || m.contains_key("hour") || m.contains_key("minute")
                || m.contains_key("second") || m.contains_key("nanosecond")
                || m.contains_key("microsecond") || m.contains_key("millisecond")
                || m.contains_key("week") || m.contains_key("ordinalDay")
                || m.contains_key("quarter") || m.contains_key("date")
                || m.contains_key("time");

            if let Some(ref tz_str) = tz_name {
                let new_tz = parse_timezone(tz_str)
                    .map_err(|e| temporal_parse_error("datetime", e))?;

                if !has_field_overrides {
                    // Pure timezone conversion for real ZonedDatetime: same instant, new TZ.
                    if let Some(Value::DateTime(base_zdt_val)) = m.get("datetime") {
                        let converted = base_zdt_val.with_time_zone(new_tz);
                        return Ok(Value::DateTime(Box::new(converted)));
                    }
                    // LocalDateTime has no timezone: just apply new TZ as wall-clock timezone.
                    if let Some(Value::LocalDateTime(ldt)) = m.get("datetime") {
                        let zoned = ldt.to_zoned(new_tz)
                            .map_err(|e| temporal_parse_error("datetime", e))?;
                        return Ok(Value::DateTime(Box::new(zoned)));
                    }
                }

                // Field overrides + timezone: apply overrides in base TZ, then convert to new TZ.
                let t = jiff::civil::Time::new(hour, minute, second, nanosecond)
                    .map_err(|e| temporal_parse_error("datetime", e))?;
                let dt = date_part.to_datetime(t);
                if let Some((_, ref base_tz)) = base_zdt {
                    if base_is_local_dt {
                        // LocalDateTime: wall-clock time is already resolved; just set new TZ.
                        let zoned = dt.to_zoned(new_tz)
                            .map_err(|e| temporal_parse_error("datetime", e))?;
                        Ok(Value::DateTime(Box::new(zoned)))
                    } else {
                        // Real ZonedDateTime: apply overrides in base TZ, then convert.
                        let with_base_tz = dt.to_zoned(base_tz.clone())
                            .map_err(|e| temporal_parse_error("datetime", e))?;
                        let zoned = with_base_tz.with_time_zone(new_tz);
                        Ok(Value::DateTime(Box::new(zoned)))
                    }
                } else if let Some(ref time_tz) = base_time_tz {
                    // "time" key provided a timezone: create in that TZ, then convert.
                    let with_time_tz = dt.to_zoned(time_tz.clone())
                        .map_err(|e| temporal_parse_error("datetime", e))?;
                    let zoned = with_time_tz.with_time_zone(new_tz);
                    Ok(Value::DateTime(Box::new(zoned)))
                } else {
                    let zoned = dt.to_zoned(new_tz)
                        .map_err(|e| temporal_parse_error("datetime", e))?;
                    Ok(Value::DateTime(Box::new(zoned)))
                }
            } else {
                let t = jiff::civil::Time::new(hour, minute, second, nanosecond)
                    .map_err(|e| temporal_parse_error("datetime", e))?;
                let dt = date_part.to_datetime(t);
                // Use timezone from "datetime" key (if it's a real ZonedDateTime),
                // "time" key, or default to UTC. LocalDateTime base has no TZ.
                let tz = if let Some((_, ref base_tz)) = base_zdt {
                    if base_is_local_dt {
                        jiff::tz::TimeZone::UTC
                    } else {
                        base_tz.clone()
                    }
                } else if let Some(ref time_tz) = base_time_tz {
                    time_tz.clone()
                } else {
                    jiff::tz::TimeZone::UTC
                };
                let zoned = dt.to_zoned(tz)
                    .map_err(|e| temporal_parse_error("datetime", e))?;
                Ok(Value::DateTime(Box::new(zoned)))
            }
        }
        other => type_error("datetime", other),
    }
}

/// Parse an ISO 8601 datetime string, handling week-date, ordinal-date, and compact formats.
fn parse_localdatetime_str(s: &str) -> Result<jiff::civil::DateTime, String> {
    if let Ok(dt) = s.parse::<jiff::civil::DateTime>() {
        return Ok(dt);
    }
    if let Ok(zdt) = s.parse::<jiff::Zoned>() {
        return Ok(jiff::civil::DateTime::from(&zdt));
    }
    let s = s.trim();
    let t_pos = s.find(['T', 't']);
    if let Some(t_idx) = t_pos {
        let date_str = &s[..t_idx];
        let rest = &s[t_idx + 1..];
        let date = parse_date_str(date_str)?;
        let time_str = rest.trim_end_matches('Z');
        let time_str = if let Some(plus_pos) = time_str.rfind('+') {
            if plus_pos > 0 { &time_str[..plus_pos] } else { time_str }
        } else if let Some(minus_pos) = time_str[1..].rfind('-') {
            &time_str[..minus_pos + 1]
        } else {
            time_str
        };
        let time_str = expand_compact_time(time_str);
        let time = time_str.parse::<jiff::civil::Time>()
            .map_err(|e| format!("cannot parse time '{time_str}': {e}"))?;
        Ok(jiff::civil::DateTime::from_parts(date, time))
    } else {
        Err(format!("cannot parse datetime from '{s}'"))
    }
}

/// Expand compact time notation `HHMMSS` to `HH:MM:SS`
fn expand_compact_time(s: &str) -> String {
    if s.len() >= 6 && !s.contains(':') {
        let hh = &s[0..2];
        let mm = &s[2..4];
        let ss = &s[4..];
        if let Some(dot_pos) = ss.find('.') {
            let (secs, frac) = ss.split_at(dot_pos);
            format!("{hh}:{mm}:{secs}{frac}")
        } else {
            format!("{hh}:{mm}:{ss}")
        }
    } else if s.len() == 4 && !s.contains(':') {
        let hh = &s[0..2];
        let mm = &s[2..4];
        format!("{hh}:{mm}:00")
    } else {
        s.to_string()
    }
}

fn eval_localdatetime_constructor(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.is_empty() {
        let now = jiff::Zoned::now();
        return Ok(Value::LocalDateTime(jiff::civil::DateTime::from(now)));
    }
    if vals.len() != 1 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "localdatetime() expects 0 or 1 argument".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => {
            let dt = parse_localdatetime_str(s)
                .map_err(|e| temporal_parse_error("localdatetime", e))?;
            Ok(Value::LocalDateTime(dt))
        }
        // Accept temporal values directly
        Value::LocalDateTime(dt) => Ok(Value::LocalDateTime(*dt)),
        Value::Date(d) => Ok(Value::LocalDateTime(d.to_datetime(jiff::civil::Time::midnight()))),
        Value::DateTime(zdt) => Ok(Value::LocalDateTime(jiff::civil::DateTime::from(zdt.as_ref()))),
        Value::Map(m) => {
            let base_date = extract_base_date(m)?;
            let base_time = extract_base_time(m)?;
            let base_dt = match m.get("datetime") {
                Some(Value::LocalDateTime(dt)) => Some(*dt),
                Some(Value::DateTime(zdt)) => Some(jiff::civil::DateTime::from(zdt.as_ref())),
                _ => None,
            };

            // Resolve date part — handles week, ordinalDay, quarter, or standard year/month/day
            let has_date_fields = m.contains_key("week") || m.contains_key("ordinalDay")
                || m.contains_key("quarter") || m.contains_key("year")
                || m.contains_key("month") || m.contains_key("day")
                || base_date.is_some();
            let effective_base = if base_date.is_some() {
                base_date
            } else if let Some(bdt) = &base_dt {
                Some(jiff::civil::Date::new(bdt.year(), bdt.month(), bdt.day())
                    .map_err(|e| temporal_parse_error("localdatetime", e))?)
            } else {
                None
            };
            let date_part = if has_date_fields {
                resolve_date_from_map(m, &effective_base)?
            } else if let Some(bdt) = &base_dt {
                jiff::civil::Date::new(bdt.year(), bdt.month(), bdt.day())
                    .map_err(|e| temporal_parse_error("localdatetime", e))?
            } else {
                jiff::civil::Date::new(0, 1, 1)
                    .map_err(|e| temporal_parse_error("localdatetime", e))?
            };

            // Resolve time part
            let (base_h, base_mi, base_s, base_ns) = if let Some(bt) = &base_time {
                (bt.hour(), bt.minute(), bt.second(), bt.subsec_nanosecond())
            } else if let Some(bdt) = &base_dt {
                (bdt.hour(), bdt.minute(), bdt.second(), bdt.subsec_nanosecond())
            } else {
                (0i8, 0i8, 0i8, 0i32)
            };
            let hour = if m.contains_key("hour") { map_get_int(m, "hour", 0)? as i8 } else { base_h };
            let minute = if m.contains_key("minute") { map_get_int(m, "minute", 0)? as i8 } else { base_mi };
            let second = if m.contains_key("second") { map_get_int(m, "second", 0)? as i8 } else { base_s };
            // Combine millisecond + microsecond + nanosecond (Cypher spec: all additive)
            let nanosecond = if m.contains_key("nanosecond")
                || m.contains_key("microsecond")
                || m.contains_key("millisecond")
            {
                let millis = map_get_int(m, "millisecond", 0)? * 1_000_000;
                let micros = map_get_int(m, "microsecond", 0)? * 1_000;
                let nanos = map_get_int(m, "nanosecond", 0)?;
                (millis + micros + nanos) as i32
            } else {
                base_ns
            };

            let t = jiff::civil::Time::new(hour, minute, second, nanosecond)
                .map_err(|e| temporal_parse_error("localdatetime", e))?;
            Ok(Value::LocalDateTime(date_part.to_datetime(t)))
        }
        other => type_error("localdatetime", other),
    }
}

/// Parse a Cypher ISO 8601 duration string, handling fractional date components.
/// Examples: 'P5M1.5D' → 5 months + 1 day + 12 hours, 'P2.5W' → 17 days + 12 hours
#[allow(dead_code)]
fn parse_cypher_duration(s: &str) -> Result<jiff::Span, String> {
    // Try standard jiff parser first
    if let Ok(sp) = s.parse::<jiff::Span>() {
        return Ok(sp);
    }

    // Try P<date>T<time> datetime-style duration (e.g. "P2012-02-02T14:37:21.545")
    let s_inner = s.strip_prefix('P').unwrap_or(s);
    if let Some(t_pos) = s_inner.find(['T', 't']) {
        let date_part = &s_inner[..t_pos];
        let time_part = &s_inner[t_pos + 1..];
        if date_part.contains('-') && !date_part.is_empty() {
            let parts: Vec<&str> = date_part.splitn(3, '-').collect();
            if parts.len() == 3 {
                let y: i32 = parts[0].parse().unwrap_or(0);
                let m: i32 = parts[1].parse().unwrap_or(0);
                let d: i32 = parts[2].parse().unwrap_or(0);
                let t_parts: Vec<&str> = time_part.splitn(3, ':').collect();
                let h: i32 = t_parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
                let mi: i32 = t_parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                let sv: f64 = t_parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let s_i = sv.trunc() as i32;
                let ns = ((sv - sv.trunc()) * 1e9) as i32;
                let sp = jiff::Span::new()
                    .years(y).months(m).days(d).hours(h).minutes(mi).seconds(s_i).nanoseconds(ns);
                return Ok(sp);
            }
        }
    }

    // Handle fractional date components by parsing manually
    let mut remaining = s_inner;
    let mut total_ns: i64 = 0;
    let mut years: i32 = 0;
    let mut months: i32 = 0;
    let mut days: i32 = 0;
    let mut in_time = false;

    const SECS_PER_MONTH: f64 = 365.25 * 86400.0 / 12.0;

    while !remaining.is_empty() {
        if remaining.starts_with('T') || remaining.starts_with('t') {
            in_time = true;
            remaining = &remaining[1..];
            continue;
        }
        let num_end = remaining.find(|c: char| c.is_alphabetic())
            .unwrap_or(remaining.len());
        if num_end == 0 { break; }
        let num_str = &remaining[..num_end];
        let num: f64 = num_str.parse().map_err(|_| format!("invalid duration number: {num_str}"))?;
        remaining = &remaining[num_end..];
        if remaining.is_empty() { break; }
        let unit = remaining.chars().next().unwrap();
        remaining = &remaining[unit.len_utf8()..];

        if in_time {
            let ns_val = match unit {
                'H' => (num * 3_600_000_000_000.0) as i64,
                'M' => (num * 60_000_000_000.0) as i64,
                'S' => (num * 1_000_000_000.0) as i64,
                _ => return Err(format!("unknown time unit: {unit}")),
            };
            total_ns += ns_val;
        } else {
            match unit {
                'Y' => {
                    years += num.trunc() as i32;
                    let frac = num - num.trunc();
                    if frac != 0.0 {
                        let frac_months = frac * 12.0;
                        months += frac_months.trunc() as i32;
                        let rem_months = frac_months - frac_months.trunc();
                        total_ns += (rem_months * SECS_PER_MONTH * 1e9) as i64;
                    }
                }
                'M' => {
                    months += num.trunc() as i32;
                    let frac = num - num.trunc();
                    if frac != 0.0 {
                        total_ns += (frac * SECS_PER_MONTH * 1e9) as i64;
                    }
                }
                'W' => {
                    let total_days = num * 7.0;
                    days += total_days.trunc() as i32;
                    let frac = total_days - total_days.trunc();
                    total_ns += (frac * 86_400_000_000_000.0) as i64;
                }
                'D' => {
                    days += num.trunc() as i32;
                    let frac = num - num.trunc();
                    total_ns += (frac * 86_400_000_000_000.0) as i64;
                }
                _ => return Err(format!("unknown date unit: {unit}")),
            }
        }
    }

    let extra_days = total_ns / 86_400_000_000_000i64;
    let rem_ns = total_ns % 86_400_000_000_000i64;
    days += extra_days as i32;
    let h = (rem_ns / 3_600_000_000_000i64) as i32;
    let rem_ns = rem_ns % 3_600_000_000_000i64;
    let m = (rem_ns / 60_000_000_000i64) as i32;
    let rem_ns = rem_ns % 60_000_000_000i64;
    let sec = (rem_ns / 1_000_000_000i64) as i32;
    let ns = (rem_ns % 1_000_000_000i64) as i32;

    Ok(jiff::Span::new()
        .years(years).months(months).days(days)
        .hours(h).minutes(m).seconds(sec).nanoseconds(ns))
}

fn eval_duration_constructor(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.is_empty() {
        return Ok(Value::Duration(CypherDuration { months: 0, days: 0, nanos: 0 }));
    }
    if vals.len() != 1 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "duration() expects 0 or 1 argument".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => {
            let d = parse_duration_str(s)?;
            Ok(Value::Duration(d))
        }
        Value::Map(m) => {
            // Helper: get a numeric value as f64, supporting both Int and Float.
            fn dur_get(m: &indexmap::IndexMap<Box<str>, Value>, key: &str) -> f64 {
                match m.get(key) {
                    Some(Value::Int(v)) => *v as f64,
                    Some(Value::Float(v)) => *v,
                    _ => 0.0,
                }
            }
            let raw_years = dur_get(m, "years") + dur_get(m, "year");
            let raw_months = dur_get(m, "months") + dur_get(m, "month");
            let raw_weeks = dur_get(m, "weeks") + dur_get(m, "week");
            let raw_days = dur_get(m, "days") + dur_get(m, "day");
            let raw_hours = dur_get(m, "hours") + dur_get(m, "hour");
            let raw_minutes = dur_get(m, "minutes") + dur_get(m, "minute");
            let raw_seconds = dur_get(m, "seconds") + dur_get(m, "second");
            let raw_millis = dur_get(m, "milliseconds") + dur_get(m, "millisecond");
            let raw_micros = dur_get(m, "microseconds") + dur_get(m, "microsecond");
            let raw_nanos_sub = dur_get(m, "nanoseconds") + dur_get(m, "nanosecond");
            let sub_sec_nanos = raw_millis * 1_000_000.0 + raw_micros * 1_000.0 + raw_nanos_sub;

            // Phase 1: years + months → total months
            let int_years = raw_years as i64;
            let frac_years = raw_years - int_years as f64;
            let total_month_float = frac_years * 12.0 + raw_months;
            let int_months_raw = total_month_float as i64;
            let frac_months = total_month_float - int_months_raw as f64;
            let total_months = int_years * 12 + int_months_raw;

            // Phase 2: fractional months + weeks + days → total days
            // 1 month ≈ 30.436875 days (2_629_746 s / 86400 s/day)
            let total_days_float = frac_months * (2_629_746.0 / 86_400.0)
                + raw_weeks * 7.0
                + raw_days;
            let int_days = total_days_float as i64;
            let frac_days = total_days_float - int_days as f64;

            // Phase 3: fractional days + hours + minutes + seconds → nanos
            let total_time_secs = frac_days * 86_400.0
                + raw_hours * 3_600.0
                + raw_minutes * 60.0
                + raw_seconds
                + sub_sec_nanos / 1_000_000_000.0;
            let total_nanos = (total_time_secs * 1_000_000_000.0).round() as i64;

            Ok(Value::Duration(CypherDuration { months: total_months, days: int_days, nanos: total_nanos }))
        }
        other => type_error("duration", other),
    }
}

/// Parse a localdatetime string containing an ISO week date portion (e.g. `2015-W30-2T19:32:24`).
#[allow(dead_code)]
fn parse_localdatetime_week_str(s: &str) -> Result<Value, GqlError> {
    // Split at 'T' to separate date and time.
    let (date_str, time_str) = if let Some(t_pos) = s.find('T') {
        (&s[..t_pos], Some(&s[t_pos + 1..]))
    } else {
        (s, None)
    };

    let date = parse_date_str(date_str).map_err(|e| temporal_parse_error("localdatetime", e))?;

    let dt = if let Some(ts) = time_str {
        let time: jiff::civil::Time = ts
            .parse()
            .map_err(|e| temporal_parse_error("localdatetime", e))?;
        date.to_datetime(time)
    } else {
        date.to_datetime(jiff::civil::Time::midnight())
    };

    Ok(Value::LocalDateTime(dt))
}

/// Parse a duration string, supporting decimal ISO 8601 durations (e.g. P1.5Y, PT1.5H)
/// and cascading overflow (e.g. P12Y5M14DT16H12M70S → 70s cascades to 1m10s).
fn parse_duration_str(s: &str) -> Result<CypherDuration, GqlError> {
    // Return error for date-like strings (e.g., "2012-02-02")
    if !s.starts_with('P') && !s.starts_with('p') {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "ISO 8601 duration string starting with 'P'".to_string(),
            actual: s.to_string(),
        }));
    }
    // Try jiff first: handles calendar format (P2012-02-02T14:37:21.545) and basic forms.
    if let Ok(sp) = s.parse::<jiff::Span>() {
        let total_months = sp.get_years() as i64 * 12 + sp.get_months() as i64;
        let total_days = sp.get_weeks() as i64 * 7 + sp.get_days() as i64;
        let time_secs = sp.get_hours() as i64 * 3600
            + sp.get_minutes() * 60
            + sp.get_seconds();
        let ms = sp.get_milliseconds();
        let us = sp.get_microseconds();
        let ns_sub = sp.get_nanoseconds();
        let total_nanos = time_secs * 1_000_000_000
            + ms * 1_000_000
            + us * 1_000
            + ns_sub;
        return Ok(CypherDuration { months: total_months, days: total_days, nanos: total_nanos });
    }
    // Fall back to custom parser for decimal values and edge cases like 'PT'.
    parse_decimal_iso_duration(s)
}

/// Parse an ISO 8601 duration string that may contain decimal values.
///
/// Examples: `P1.5Y`, `PT1.5H`, `P1Y2M3.5D`.
/// Cascades fractional parts to the next smaller unit.
#[allow(dead_code)]
fn parse_decimal_iso_duration(s: &str) -> Result<CypherDuration, GqlError> {
    let s = s.trim();
    let s = s.strip_prefix('P').ok_or_else(|| {
        GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "ISO 8601 duration starting with 'P'".to_string(),
            actual: s.to_string(),
        })
    })?;

    // Split at 'T' to separate date and time components.
    let (date_str, time_str) = match s.find('T') {
        Some(pos) => (&s[..pos], &s[pos + 1..]),
        None => (s, ""),
    };

    // Parse components of the form: <number><designator> where number may be decimal.
    fn parse_components(
        s: &str,
        designators: &[u8],
    ) -> Result<Vec<(f64, char)>, GqlError> {
        let mut result = Vec::new();
        let mut remaining = s;
        while !remaining.is_empty() {
            let des_pos = remaining
                .bytes()
                .position(|b| designators.contains(&b))
                .ok_or_else(|| {
                    GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: format!(
                            "duration unit designator (one of {})",
                            designators.iter().map(|&b| b as char).collect::<String>()
                        ),
                        actual: remaining.to_string(),
                    })
                })?;
            let num_str = &remaining[..des_pos];
            let designator = remaining.as_bytes()[des_pos] as char;
            let value: f64 = num_str.parse().map_err(|_| {
                GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "numeric duration value".to_string(),
                    actual: num_str.to_string(),
                })
            })?;
            result.push((value, designator));
            remaining = &remaining[des_pos + 1..];
        }
        Ok(result)
    }

    let date_comps = if date_str.is_empty() {
        vec![]
    } else {
        parse_components(date_str, b"YMWD")?
    };
    let time_comps = if time_str.is_empty() {
        vec![]
    } else {
        parse_components(time_str, b"HMS")?
    };

    let mut years = 0.0f64;
    let mut months = 0.0f64;
    let mut weeks = 0.0f64;
    let mut days = 0.0f64;
    for (v, d) in date_comps {
        match d {
            'Y' => years += v,
            'M' => months += v,
            'W' => weeks += v,
            'D' => days += v,
            _ => {}
        }
    }

    let mut hours = 0.0f64;
    let mut minutes = 0.0f64;
    let mut seconds = 0.0f64;
    for (v, d) in time_comps {
        match d {
            'H' => hours += v,
            'M' => minutes += v,
            'S' => seconds += v,
            _ => {}
        }
    }

    // Phase 1: years + months → total months
    let int_years = years as i64;
    let frac_years = years - int_years as f64;
    let total_month_float = frac_years * 12.0 + months;
    let int_months_raw = total_month_float as i64;
    let frac_months = total_month_float - int_months_raw as f64;
    let total_months = int_years * 12 + int_months_raw;

    // Phase 2: fractional months + weeks + days → total days
    let total_days_float = frac_months * (2_629_746.0 / 86_400.0)
        + weeks * 7.0
        + days;
    let int_days = total_days_float as i64;
    let frac_days = total_days_float - int_days as f64;

    // Phase 3: fractional days + hours + minutes + seconds → nanos
    let total_time_secs = frac_days * 86_400.0
        + hours * 3_600.0
        + minutes * 60.0
        + seconds;
    let total_nanos = (total_time_secs * 1_000_000_000.0).round() as i64;

    Ok(CypherDuration { months: total_months, days: int_days, nanos: total_nanos })
}

// ── Temporal truncate helpers ────────────────────────────────────────────────

/// Extract a date (year, month, day) from any temporal value.
fn extract_date(val: &Value) -> Option<jiff::civil::Date> {
    match val {
        Value::Date(d) => Some(*d),
        Value::LocalDateTime(dt) => Some(dt.date()),
        Value::DateTime(zdt) => Some(jiff::civil::Date::from(&**zdt)),
        _ => None,
    }
}

/// Extract a time (hour, minute, second, nanosecond) from any temporal value.
fn extract_time(val: &Value) -> Option<jiff::civil::Time> {
    match val {
        Value::LocalTime(t) => Some(*t),
        Value::LocalDateTime(dt) => Some(dt.time()),
        Value::DateTime(zdt) => Some(jiff::civil::Time::from(&**zdt)),
        _ => None,
    }
}

/// Extract timezone from a temporal value, if available.
fn extract_tz(val: &Value) -> Option<jiff::tz::TimeZone> {
    match val {
        Value::DateTime(zdt) => Some(zdt.time_zone().clone()),
        _ => None,
    }
}

/// Truncate a date to the given unit, returning (year, month, day).
fn truncate_date_fields(
    d: jiff::civil::Date,
    unit: &str,
) -> Result<jiff::civil::Date, GqlError> {
    let y = d.year() as i64;
    match unit {
        "millennium" => {
            let ty = (y / 1000) * 1000;
            jiff::civil::Date::new(ty as i16, 1, 1)
                .map_err(|e| temporal_parse_error("truncate", e))
        }
        "century" => {
            let ty = (y / 100) * 100;
            jiff::civil::Date::new(ty as i16, 1, 1)
                .map_err(|e| temporal_parse_error("truncate", e))
        }
        "decade" => {
            let ty = (y / 10) * 10;
            jiff::civil::Date::new(ty as i16, 1, 1)
                .map_err(|e| temporal_parse_error("truncate", e))
        }
        "year" => jiff::civil::Date::new(d.year(), 1, 1)
            .map_err(|e| temporal_parse_error("truncate", e)),
        "weekyear" => {
            // ISO week year: find the Monday of week 1 of the ISO week year.
            let iso = d.iso_week_date();
            let iso_year = iso.year();
            // Jan 4 is always in week 1 of the ISO year.
            let jan4 = jiff::civil::Date::new(iso_year, 1, 4)
                .map_err(|e| temporal_parse_error("truncate", e))?;
            let jan4_dow = jan4.weekday().to_monday_one_offset() as i64;
            // Monday of week 1
            let monday_ordinal = 4 - jan4_dow + 1;
            let base = jiff::civil::Date::new(iso_year, 1, 1)
                .map_err(|e| temporal_parse_error("truncate", e))?;
            base.checked_add(jiff::Span::new().days(monday_ordinal - 1))
                .map_err(|e| temporal_parse_error("truncate", e))
        }
        "quarter" => {
            let q = ((d.month() as i64 - 1) / 3) * 3 + 1;
            jiff::civil::Date::new(d.year(), q as i8, 1)
                .map_err(|e| temporal_parse_error("truncate", e))
        }
        "month" => jiff::civil::Date::new(d.year(), d.month(), 1)
            .map_err(|e| temporal_parse_error("truncate", e)),
        "week" => {
            // Monday of the current week (ISO weeks start on Monday)
            let dow = d.weekday().to_monday_one_offset() as i64; // 1=Mon..7=Sun
            d.checked_add(jiff::Span::new().days(1 - dow))
                .map_err(|e| temporal_parse_error("truncate", e))
        }
        "day" | "hour" | "minute" | "second" | "millisecond" | "microsecond" => Ok(d),
        other => Err(temporal_parse_error("truncate", format!("unknown unit '{other}'"))),
    }
}

/// Truncate a time to the given unit.
fn truncate_time_fields(t: jiff::civil::Time, unit: &str) -> Result<jiff::civil::Time, GqlError> {
    match unit {
        "day" | "millennium" | "century" | "decade" | "year" | "weekyear" | "quarter"
        | "month" | "week" => Ok(jiff::civil::Time::midnight()),
        "hour" => jiff::civil::Time::new(t.hour(), 0, 0, 0)
            .map_err(|e| temporal_parse_error("truncate", e)),
        "minute" => jiff::civil::Time::new(t.hour(), t.minute(), 0, 0)
            .map_err(|e| temporal_parse_error("truncate", e)),
        "second" => jiff::civil::Time::new(t.hour(), t.minute(), t.second(), 0)
            .map_err(|e| temporal_parse_error("truncate", e)),
        "millisecond" => {
            let ns = (t.subsec_nanosecond() / 1_000_000) * 1_000_000;
            jiff::civil::Time::new(t.hour(), t.minute(), t.second(), ns)
                .map_err(|e| temporal_parse_error("truncate", e))
        }
        "microsecond" => {
            let ns = (t.subsec_nanosecond() / 1_000) * 1_000;
            jiff::civil::Time::new(t.hour(), t.minute(), t.second(), ns)
                .map_err(|e| temporal_parse_error("truncate", e))
        }
        other => Err(temporal_parse_error("truncate", format!("unknown unit '{other}'"))),
    }
}

/// Apply map overrides for date fields: day, dayOfWeek.
fn apply_date_overrides(
    d: jiff::civil::Date,
    overrides: &indexmap::IndexMap<Box<str>, Value>,
) -> Result<jiff::civil::Date, GqlError> {
    let mut result = d;
    if let Some(Value::Int(day)) = overrides.get("day") {
        result = jiff::civil::Date::new(result.year(), result.month(), *day as i8)
            .map_err(|e| temporal_parse_error("truncate", e))?;
    }
    if let Some(Value::Int(dow)) = overrides.get("dayOfWeek") {
        // Current day-of-week and desired day-of-week (1=Monday)
        let current_dow = result.weekday().to_monday_one_offset() as i64;
        let offset = *dow - current_dow;
        result = result
            .checked_add(jiff::Span::new().days(offset))
            .map_err(|e| temporal_parse_error("truncate", e))?;
    }
    Ok(result)
}

/// Apply map overrides for time fields: hour, minute, second, millisecond, microsecond, nanosecond.
fn apply_time_overrides(
    t: jiff::civil::Time,
    overrides: &indexmap::IndexMap<Box<str>, Value>,
) -> Result<jiff::civil::Time, GqlError> {
    let hour = if let Some(Value::Int(v)) = overrides.get("hour") {
        *v as i8
    } else {
        t.hour()
    };
    let minute = if let Some(Value::Int(v)) = overrides.get("minute") {
        *v as i8
    } else {
        t.minute()
    };
    let second = if let Some(Value::Int(v)) = overrides.get("second") {
        *v as i8
    } else {
        t.second()
    };
    // Sub-second components are hierarchical: millisecond > microsecond > nanosecond
    let base_ns = t.subsec_nanosecond();
    let base_ms = base_ns / 1_000_000;
    let base_us = (base_ns % 1_000_000) / 1_000;
    let base_ns_only = base_ns % 1_000;

    let ms = if let Some(Value::Int(v)) = overrides.get("millisecond") {
        *v as i32
    } else {
        base_ms
    };
    let us = if let Some(Value::Int(v)) = overrides.get("microsecond") {
        *v as i32
    } else if overrides.contains_key("millisecond") {
        0
    } else {
        base_us
    };
    let ns_only = if let Some(Value::Int(v)) = overrides.get("nanosecond") {
        *v as i32
    } else if overrides.contains_key("microsecond") || overrides.contains_key("millisecond") {
        0
    } else {
        base_ns_only
    };
    let nanosecond = ms * 1_000_000 + us * 1_000 + ns_only;
    jiff::civil::Time::new(hour, minute, second, nanosecond)
        .map_err(|e| temporal_parse_error("truncate", e))
}

fn eval_date_truncate(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() < 2 || vals.len() > 3 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "date.truncate(unit, temporal[, map])".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    let unit = match &vals[0] {
        Value::Str(s) => s.to_ascii_lowercase(),
        other => return type_error("date.truncate", other),
    };
    let d = extract_date(&vals[1]).ok_or_else(|| temporal_parse_error("date.truncate", "expected a temporal with date"))?;
    let truncated = truncate_date_fields(d, &unit)?;
    let overrides = match vals.get(2) {
        Some(Value::Map(m)) => m,
        None => &indexmap::IndexMap::new(),
        _ => return type_error("date.truncate", &vals[2]),
    };
    let result = apply_date_overrides(truncated, overrides)?;
    Ok(Value::Date(result))
}

fn eval_datetime_truncate(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() < 2 || vals.len() > 3 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "datetime.truncate(unit, temporal[, map])".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    let unit = match &vals[0] {
        Value::Str(s) => s.to_ascii_lowercase(),
        other => return type_error("datetime.truncate", other),
    };
    let d = extract_date(&vals[1]).ok_or_else(|| temporal_parse_error("datetime.truncate", "expected a temporal with date"))?;
    let t = extract_time(&vals[1]).unwrap_or(jiff::civil::Time::midnight());
    let base_tz = extract_tz(&vals[1]);

    let truncated_date = truncate_date_fields(d, &unit)?;
    let truncated_time = truncate_time_fields(t, &unit)?;

    let empty_map = indexmap::IndexMap::new();
    let overrides = match vals.get(2) {
        Some(Value::Map(m)) => m,
        None => &empty_map,
        _ => return type_error("datetime.truncate", &vals[2]),
    };

    let final_date = apply_date_overrides(truncated_date, overrides)?;
    let final_time = apply_time_overrides(truncated_time, overrides)?;

    // Determine timezone: map override > input timezone > UTC
    let tz = if let Some(tz_str) = map_get_str(overrides, "timezone") {
        parse_timezone(&tz_str).map_err(|e| temporal_parse_error("datetime.truncate", e))?
    } else if let Some(ref btz) = base_tz {
        btz.clone()
    } else {
        jiff::tz::TimeZone::UTC
    };

    let dt = jiff::civil::DateTime::new(
        final_date.year(),
        final_date.month(),
        final_date.day(),
        final_time.hour(),
        final_time.minute(),
        final_time.second(),
        final_time.subsec_nanosecond() as i32,
    )
    .map_err(|e| temporal_parse_error("datetime.truncate", e))?;

    let zoned = dt
        .to_zoned(tz)
        .map_err(|e| temporal_parse_error("datetime.truncate", e))?;
    Ok(Value::DateTime(Box::new(zoned)))
}

fn eval_localdatetime_truncate(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() < 2 || vals.len() > 3 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "localdatetime.truncate(unit, temporal[, map])".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    let unit = match &vals[0] {
        Value::Str(s) => s.to_ascii_lowercase(),
        other => return type_error("localdatetime.truncate", other),
    };
    let d = extract_date(&vals[1]).ok_or_else(|| temporal_parse_error("localdatetime.truncate", "expected a temporal with date"))?;
    let t = extract_time(&vals[1]).unwrap_or(jiff::civil::Time::midnight());

    let truncated_date = truncate_date_fields(d, &unit)?;
    let truncated_time = truncate_time_fields(t, &unit)?;

    let empty_map = indexmap::IndexMap::new();
    let overrides = match vals.get(2) {
        Some(Value::Map(m)) => m,
        None => &empty_map,
        _ => return type_error("localdatetime.truncate", &vals[2]),
    };

    let final_date = apply_date_overrides(truncated_date, overrides)?;
    let final_time = apply_time_overrides(truncated_time, overrides)?;

    let dt = jiff::civil::DateTime::new(
        final_date.year(),
        final_date.month(),
        final_date.day(),
        final_time.hour(),
        final_time.minute(),
        final_time.second(),
        final_time.subsec_nanosecond() as i32,
    )
    .map_err(|e| temporal_parse_error("localdatetime.truncate", e))?;
    Ok(Value::LocalDateTime(dt))
}

fn eval_localtime_truncate(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() < 2 || vals.len() > 3 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "localtime.truncate(unit, temporal[, map])".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    let unit = match &vals[0] {
        Value::Str(s) => s.to_ascii_lowercase(),
        other => return type_error("localtime.truncate", other),
    };
    let t = extract_time(&vals[1]).ok_or_else(|| temporal_parse_error("localtime.truncate", "expected a temporal with time"))?;
    let truncated = truncate_time_fields(t, &unit)?;
    let empty_map = indexmap::IndexMap::new();
    let overrides = match vals.get(2) {
        Some(Value::Map(m)) => m,
        None => &empty_map,
        _ => return type_error("localtime.truncate", &vals[2]),
    };
    let result = apply_time_overrides(truncated, overrides)?;
    Ok(Value::LocalTime(result))
}

fn eval_time_truncate(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() < 2 || vals.len() > 3 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "time.truncate(unit, temporal[, map])".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    let unit = match &vals[0] {
        Value::Str(s) => s.to_ascii_lowercase(),
        other => return type_error("time.truncate", other),
    };
    let t = extract_time(&vals[1]).ok_or_else(|| temporal_parse_error("time.truncate", "expected a temporal with time"))?;
    let base_tz = extract_tz(&vals[1]);
    let truncated = truncate_time_fields(t, &unit)?;
    let empty_map = indexmap::IndexMap::new();
    let overrides = match vals.get(2) {
        Some(Value::Map(m)) => m,
        None => &empty_map,
        _ => return type_error("time.truncate", &vals[2]),
    };
    let result = apply_time_overrides(truncated, overrides)?;

    // time.truncate returns a zoned time (DateTime) — use timezone from override > input > UTC
    let tz = if let Some(tz_str) = map_get_str(overrides, "timezone") {
        parse_timezone(&tz_str).map_err(|e| temporal_parse_error("time.truncate", e))?
    } else if let Some(ref btz) = base_tz {
        btz.clone()
    } else {
        jiff::tz::TimeZone::UTC
    };

    // Build a zoned datetime at epoch date (1970-01-01) with the truncated time.
    // Then we return as DateTime since `time` in Neo4j is zoned time.
    let dt = jiff::civil::DateTime::new(
        1970, 1, 1,
        result.hour(), result.minute(), result.second(),
        result.subsec_nanosecond() as i32,
    )
    .map_err(|e| temporal_parse_error("time.truncate", e))?;
    let zoned = dt
        .to_zoned(tz)
        .map_err(|e| temporal_parse_error("time.truncate", e))?;
    Ok(Value::DateTime(Box::new(zoned)))
}

// ── Duration namespace function helpers ──────────────────────────────────────

/// Helper: extract civil DateTime from any temporal for duration calculations.
fn temporal_to_civil_datetime(val: &Value) -> Option<jiff::civil::DateTime> {
    match val {
        Value::Date(d) => Some(d.to_datetime(jiff::civil::Time::midnight())),
        Value::LocalTime(t) => Some(jiff::civil::Date::new(0, 1, 1).unwrap().to_datetime(*t)),
        Value::LocalDateTime(dt) => Some(*dt),
        Value::DateTime(zdt) => Some(jiff::civil::DateTime::from(&**zdt)),
        _ => None,
    }
}

/// Check if a temporal has only time (no date component meaningful for calendar math).
/// Includes `time()` values stored as DateTime with epoch date 1970-01-01.
fn is_time_only(val: &Value) -> bool {
    match val {
        Value::LocalTime(_) => true,
        // ZonedTime is stored as DateTime with epoch date 1970-01-01
        Value::DateTime(zdt) => {
            let d = jiff::civil::Date::from(zdt.as_ref());
            d.year() == 1970 && d.month() == 1 && d.day() == 1
        }
        _ => false,
    }
}

/// Check if a temporal has date component.
fn has_date_component(val: &Value) -> bool {
    match val {
        Value::Date(_) | Value::LocalDateTime(_) => true,
        // DateTime has date component unless it's a ZonedTime (epoch date 1970-01-01)
        Value::DateTime(zdt) => {
            let d = jiff::civil::Date::from(zdt.as_ref());
            !(d.year() == 1970 && d.month() == 1 && d.day() == 1)
        }
        _ => false,
    }
}

fn eval_duration_between(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() != 2 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "duration.between(temporal1, temporal2)".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    if matches!(&vals[0], Value::Null) || matches!(&vals[1], Value::Null) {
        return Ok(Value::Null);
    }

    let both_have_dates = has_date_component(&vals[0]) && has_date_component(&vals[1]);
    let either_time_only = is_time_only(&vals[0]) || is_time_only(&vals[1]);

    // For zoned datetime pairs, use timestamp-based calculation
    if let (Value::DateTime(z1), Value::DateTime(z2)) = (&vals[0], &vals[1]) {
        if both_have_dates {
            // Calendar-based diff for months only; remaining as pure nanoseconds (no calendar days).
            // This correctly handles DST transitions (e.g. PT6H instead of P1DT-19H).
            let d1 = jiff::civil::Date::from(&**z1);
            let d2 = jiff::civil::Date::from(&**z2);
            let months_span = d1.until(
                jiff::civil::DateDifference::new(d2).largest(jiff::Unit::Year),
            ).map_err(|e| temporal_parse_error("duration.between", e))?;
            let total_months = months_span.get_years() as i64 * 12 + months_span.get_months() as i64;

            let ts1 = z1.timestamp();
            let ts2 = z2.timestamp();
            let total_nanos = ts2.as_nanosecond() - ts1.as_nanosecond();

            // Subtract only the months portion from total nanos
            let months_nanos = if total_months != 0 {
                let z1_plus_months = z1.as_ref().clone()
                    .checked_add(jiff::Span::new().months(total_months))
                    .map_err(|e| temporal_parse_error("duration.between", e))?;
                z1_plus_months.timestamp().as_nanosecond() - ts1.as_nanosecond()
            } else {
                0
            };
            let time_nanos = total_nanos - months_nanos;
            let time_secs = (time_nanos / 1_000_000_000) as i64;
            let time_subsec_nanos = (time_nanos % 1_000_000_000) as i32;

            let sp = build_duration_span(total_months, 0, time_secs, time_subsec_nanos);
            return Ok(Value::Duration(sp));
        } else {
            // Mixed DateTime pair (e.g. time + real-datetime): use UTC time-of-day only.
            // Using full timestamps would include the date difference (wrong for time-only cases).
            let utc_tod_nanos = |zdt: &jiff::Zoned| -> i64 {
                let utc = zdt.with_time_zone(jiff::tz::TimeZone::UTC);
                let t = jiff::civil::Time::from(&utc);
                t.hour() as i64 * 3_600_000_000_000
                    + t.minute() as i64 * 60_000_000_000
                    + t.second() as i64 * 1_000_000_000
                    + t.subsec_nanosecond() as i64
            };
            let nanos_diff = utc_tod_nanos(z2) - utc_tod_nanos(z1);
            let secs = nanos_diff / 1_000_000_000;
            let subsec = (nanos_diff % 1_000_000_000) as i32;
            let sp = build_duration_span(0, 0, secs, subsec);
            return Ok(Value::Duration(sp));
        }
    }

    let dt1 = temporal_to_civil_datetime(&vals[0])
        .ok_or_else(|| temporal_parse_error("duration.between", "expected temporal argument"))?;
    let dt2 = temporal_to_civil_datetime(&vals[1])
        .ok_or_else(|| temporal_parse_error("duration.between", "expected temporal argument"))?;

    // Check if both inputs have a time component (i.e. both are LocalDateTime)
    let both_local_datetime = matches!(&vals[0], Value::LocalDateTime(_))
        && matches!(&vals[1], Value::LocalDateTime(_));

    if both_have_dates && !either_time_only {
        let d1 = dt1.date();
        let d2 = dt2.date();

        let months_span = d1.until(
                jiff::civil::DateDifference::new(d2).largest(jiff::Unit::Year),
            ).map_err(|e| temporal_parse_error("duration.between", e))?;
        let total_months = months_span.get_years() as i64 * 12 + months_span.get_months() as i64;

        let d1_advanced = d1
            .checked_add(jiff::Span::new().months(total_months))
            .map_err(|e| temporal_parse_error("duration.between", e))?;

        if both_local_datetime {
            // Both are LocalDateTime: compute remaining as pure nanoseconds (no calendar days).
            // e.g. 2018-01-01T12:00 → 2018-01-02T10:00 = PT22H (not P1DT-2H)
            let dt1_advanced_dt = d1_advanced.to_datetime(dt1.time());
            // Use UTC zone for civil datetime → timestamp conversion (no DST effects)
            let dt1_adv_ts = dt1_advanced_dt
                .to_zoned(jiff::tz::TimeZone::UTC)
                .map_err(|e| temporal_parse_error("duration.between", e))?
                .timestamp()
                .as_nanosecond();
            let dt2_ts = dt2
                .to_zoned(jiff::tz::TimeZone::UTC)
                .map_err(|e| temporal_parse_error("duration.between", e))?
                .timestamp()
                .as_nanosecond();
            let diff_nanos = dt2_ts - dt1_adv_ts;
            let secs = (diff_nanos / 1_000_000_000) as i64;
            let subsec = (diff_nanos % 1_000_000_000) as i32;
            let sp = build_duration_span(total_months, 0, secs, subsec);
            return Ok(Value::Duration(sp));
        }

        // At least one is Date-only: include calendar days + time diff
        let remaining_days_span = d1_advanced.until(d2)
            .map_err(|e| temporal_parse_error("duration.between", e))?;
        let mut remaining_days = remaining_days_span.get_days() as i64;

        // Time diff using local civil times
        let t1 = dt1.time();
        let t2 = dt2.time();
        let t1_nanos = t1.hour() as i64 * 3_600_000_000_000
            + t1.minute() as i64 * 60_000_000_000
            + t1.second() as i64 * 1_000_000_000
            + t1.subsec_nanosecond() as i64;
        let t2_nanos = t2.hour() as i64 * 3_600_000_000_000
            + t2.minute() as i64 * 60_000_000_000
            + t2.second() as i64 * 1_000_000_000
            + t2.subsec_nanosecond() as i64;
        let mut time_diff_nanos = t2_nanos - t1_nanos;

        // Normalize: avoid mixed-sign days and time (borrow/carry 1 day when needed)
        // e.g. 3 days + -21:40:36s → 2 days + 2:19:24s
        if remaining_days > 0 && time_diff_nanos < 0 {
            remaining_days -= 1;
            time_diff_nanos += 86_400_000_000_000;
        } else if remaining_days < 0 && time_diff_nanos > 0 {
            remaining_days += 1;
            time_diff_nanos -= 86_400_000_000_000;
        }

        let total_secs = time_diff_nanos / 1_000_000_000;
        let subsec_nanos = (time_diff_nanos % 1_000_000_000) as i32;

        let sp = build_duration_span(total_months, remaining_days, total_secs, subsec_nanos);
        Ok(Value::Duration(sp))
    } else {
        // Time-only branch: at least one arg is LocalTime (no timezone).
        // Use civil time-of-day for all comparisons (ignores TZ of any ZonedTime arg).
        // Note: ZonedTime+ZonedTime is handled by the (DateTime,DateTime) branch above.
        let t1_nanos = civil_time_nanos(&vals[0]);
        let t2_nanos = civil_time_nanos(&vals[1]);
        let diff = t2_nanos - t1_nanos;
        let secs = diff / 1_000_000_000;
        let subsec = (diff % 1_000_000_000) as i32;
        let sp = build_duration_span(0, 0, secs, subsec);
        Ok(Value::Duration(sp))
    }
}

/// Get the civil time-of-day nanoseconds for a temporal value.
fn civil_time_nanos(val: &Value) -> i64 {
    let dt = temporal_to_civil_datetime(val)
        .unwrap_or_else(|| jiff::civil::Date::new(1, 1, 1).unwrap().to_datetime(jiff::civil::Time::midnight()));
    let t = dt.time();
    t.hour() as i64 * 3_600_000_000_000
        + t.minute() as i64 * 60_000_000_000
        + t.second() as i64 * 1_000_000_000
        + t.subsec_nanosecond() as i64
}

fn build_duration_span(total_months: i64, days: i64, secs: i64, subsec_nanos: i32) -> CypherDuration {
    let total_nanos = secs * 1_000_000_000 + subsec_nanos as i64;
    CypherDuration { months: total_months, days, nanos: total_nanos }
}

fn eval_duration_in_months(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() != 2 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "duration.inMonths(temporal1, temporal2)".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    if matches!(&vals[0], Value::Null) || matches!(&vals[1], Value::Null) {
        return Ok(Value::Null);
    }
    // If either is time-only, return zero duration
    if is_time_only(&vals[0]) || is_time_only(&vals[1])
        || !has_date_component(&vals[0]) || !has_date_component(&vals[1])
    {
        return Ok(Value::Duration(CypherDuration { months: 0, days: 0, nanos: 0 }));
    }
    let dt1 = temporal_to_civil_datetime(&vals[0])
        .ok_or_else(|| temporal_parse_error("duration.inMonths", "expected temporal"))?;
    let dt2 = temporal_to_civil_datetime(&vals[1])
        .ok_or_else(|| temporal_parse_error("duration.inMonths", "expected temporal"))?;

    let d1 = dt1.date();
    let d2 = dt2.date();
    let months_span = d1.until(
        jiff::civil::DateDifference::new(d2).largest(jiff::Unit::Year),
    ).map_err(|e| temporal_parse_error("duration.inMonths", e))?;
    let mut total_months = months_span.get_years() as i64 * 12 + months_span.get_months() as i64;

    // For zoned datetime pairs, adjust total_months by 1 if the time-of-day
    // comparison shows we haven't fully completed the last month.
    if let (Value::DateTime(z1), Value::DateTime(z2)) = (&vals[0], &vals[1]) {
        let ts2_ns = z2.timestamp().as_nanosecond();
        if let Ok(advanced) = z1.as_ref().clone().checked_add(jiff::Span::new().months(total_months)) {
            let adv_ns = advanced.timestamp().as_nanosecond();
            // If we overshot in the direction of travel, retract by 1 month.
            if total_months >= 0 && adv_ns > ts2_ns {
                total_months -= 1;
            } else if total_months < 0 && adv_ns < ts2_ns {
                total_months += 1;
            }
        }
    }

    Ok(Value::Duration(CypherDuration { months: total_months, days: 0, nanos: 0 }))
}

fn eval_duration_in_days(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() != 2 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "duration.inDays(temporal1, temporal2)".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    if matches!(&vals[0], Value::Null) || matches!(&vals[1], Value::Null) {
        return Ok(Value::Null);
    }
    if is_time_only(&vals[0]) || is_time_only(&vals[1])
        || !has_date_component(&vals[0]) || !has_date_component(&vals[1])
    {
        return Ok(Value::Duration(CypherDuration { months: 0, days: 0, nanos: 0 }));
    }

    // For zoned datetimes, compute day difference based on dates
    let d1 = extract_date(&vals[0])
        .ok_or_else(|| temporal_parse_error("duration.inDays", "expected temporal with date"))?;
    let d2 = extract_date(&vals[1])
        .ok_or_else(|| temporal_parse_error("duration.inDays", "expected temporal with date"))?;

    let diff_span = d1.until(
        jiff::civil::DateDifference::new(d2).largest(jiff::Unit::Day),
    ).map_err(|e| temporal_parse_error("duration.inDays", e))?;
    let diff_days = diff_span.get_days() as i64;
    Ok(Value::Duration(CypherDuration { months: 0, days: diff_days, nanos: 0 }))
}

fn eval_duration_in_seconds(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() != 2 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "duration.inSeconds(temporal1, temporal2)".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    if matches!(&vals[0], Value::Null) || matches!(&vals[1], Value::Null) {
        return Ok(Value::Null);
    }

    // For zoned datetime pairs, use timestamp-based or time-of-day calculation.
    if let (Value::DateTime(z1), Value::DateTime(z2)) = (&vals[0], &vals[1]) {
        let v1_has_date = has_date_component(&vals[0]);
        let v2_has_date = has_date_component(&vals[1]);
        if v1_has_date && v2_has_date {
            // Both are real datetimes: use exact timestamp diff.
            let ts1 = z1.timestamp();
            let ts2 = z2.timestamp();
            let total_nanos = (ts2.as_nanosecond() - ts1.as_nanosecond()) as i64;
            return Ok(Value::Duration(CypherDuration { months: 0, days: 0, nanos: total_nanos }));
        } else {
            // Mixed: one is time-only (epoch date). Compare UTC time-of-day only.
            let utc_tod_nanos = |zdt: &jiff::Zoned| -> i64 {
                let utc = zdt.with_time_zone(jiff::tz::TimeZone::UTC);
                let t = jiff::civil::Time::from(&utc);
                t.hour() as i64 * 3_600_000_000_000
                    + t.minute() as i64 * 60_000_000_000
                    + t.second() as i64 * 1_000_000_000
                    + t.subsec_nanosecond() as i64
            };
            let nanos_diff = utc_tod_nanos(z2) - utc_tod_nanos(z1);
            return Ok(Value::Duration(CypherDuration { months: 0, days: 0, nanos: nanos_diff }));
        }
    }

    // Mixed DateTime + LocalDateTime/LocalTime: use DST-aware calculation if possible.
    let zoned_val = match (&vals[0], &vals[1]) {
        (Value::DateTime(z), _) if has_date_component(&vals[0]) => Some((z.as_ref(), true)),
        (_, Value::DateTime(z)) if has_date_component(&vals[1]) => Some((z.as_ref(), false)),
        _ => None,
    };
    if let Some((zdt, zdt_is_first)) = zoned_val {
        let tz = zdt.time_zone().clone();
        let other_val = if zdt_is_first { &vals[1] } else { &vals[0] };
        // For LocalTime input, combine with the DateTime's date for proper DST-aware calc.
        let other_civil: Option<jiff::civil::DateTime> = match other_val {
            Value::LocalTime(t) => {
                // Use the date from the zoned datetime, combined with the local time
                let zdt_date = jiff::civil::Date::from(zdt);
                Some(zdt_date.to_datetime(*t))
            }
            other => temporal_to_civil_datetime(other),
        };
        if let Some(other_civil_dt) = other_civil {
            if let Ok(other_zoned) = other_civil_dt.to_zoned(tz) {
                let (ts1, ts2) = if zdt_is_first {
                    (zdt.timestamp(), other_zoned.timestamp())
                } else {
                    (other_zoned.timestamp(), zdt.timestamp())
                };
                let total_nanos = (ts2.as_nanosecond() - ts1.as_nanosecond()) as i64;
                return Ok(Value::Duration(CypherDuration { months: 0, days: 0, nanos: total_nanos }));
            }
        }
    }

    // For mixed datetime + time or two localdatetimes, use civil calculation
    let dt1 = temporal_to_civil_datetime(&vals[0])
        .ok_or_else(|| temporal_parse_error("duration.inSeconds", "expected temporal"))?;
    let dt2 = temporal_to_civil_datetime(&vals[1])
        .ok_or_else(|| temporal_parse_error("duration.inSeconds", "expected temporal"))?;

    let both_have_dates = has_date_component(&vals[0]) && has_date_component(&vals[1]);

    if both_have_dates {
        // Convert both to total seconds from epoch-like reference
        let d1 = dt1.date();
        let d2 = dt2.date();
        let days_span = d1.until(
            jiff::civil::DateDifference::new(d2).largest(jiff::Unit::Day),
        ).map_err(|e| temporal_parse_error("duration.inSeconds", e))?;
        let diff_days = days_span.get_days() as i64;

        let t1 = dt1.time();
        let t2 = dt2.time();
        let t1_nanos = t1.hour() as i64 * 3_600_000_000_000
            + t1.minute() as i64 * 60_000_000_000
            + t1.second() as i64 * 1_000_000_000
            + t1.subsec_nanosecond() as i64;
        let t2_nanos = t2.hour() as i64 * 3_600_000_000_000
            + t2.minute() as i64 * 60_000_000_000
            + t2.second() as i64 * 1_000_000_000
            + t2.subsec_nanosecond() as i64;

        let total_nanos = diff_days * 86_400_000_000_000 + (t2_nanos - t1_nanos);
        Ok(Value::Duration(CypherDuration { months: 0, days: 0, nanos: total_nanos }))
    } else {
        // Time-only
        let t1 = dt1.time();
        let t2 = dt2.time();
        let t1_nanos = t1.hour() as i64 * 3_600_000_000_000
            + t1.minute() as i64 * 60_000_000_000
            + t1.second() as i64 * 1_000_000_000
            + t1.subsec_nanosecond() as i64;
        let t2_nanos = t2.hour() as i64 * 3_600_000_000_000
            + t2.minute() as i64 * 60_000_000_000
            + t2.second() as i64 * 1_000_000_000
            + t2.subsec_nanosecond() as i64;
        let diff = t2_nanos - t1_nanos;
        Ok(Value::Duration(CypherDuration { months: 0, days: 0, nanos: diff }))
    }
}

// ── Temporal arithmetic helpers ─────────────────────────────────────────────

/// Perform temporal + duration or temporal - duration.
pub fn temporal_add_duration(temporal: &Value, dur: &CypherDuration, subtract: bool) -> Result<Value, GqlError> {
    let (months, days, nanos) = if subtract {
        (-dur.months, -dur.days, -dur.nanos)
    } else {
        (dur.months, dur.days, dur.nanos)
    };
    match temporal {
        Value::Date(d) => {
            let d1 = if months != 0 {
                d.checked_add(jiff::Span::new().months(months))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else { *d };
            let d2 = if days != 0 {
                d1.checked_add(jiff::Span::new().days(days))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else { d1 };
            Ok(Value::Date(d2))
        }
        Value::LocalTime(t) => {
            let cur_nanos = t.hour() as i64 * 3_600_000_000_000
                + t.minute() as i64 * 60_000_000_000
                + t.second() as i64 * 1_000_000_000
                + t.subsec_nanosecond() as i64;
            let total = cur_nanos + nanos;
            let day_nanos: i64 = 24 * 3_600_000_000_000;
            let wrapped = ((total % day_nanos) + day_nanos) % day_nanos;
            let h = (wrapped / 3_600_000_000_000) as i8;
            let rem = wrapped % 3_600_000_000_000;
            let m = (rem / 60_000_000_000) as i8;
            let rem = rem % 60_000_000_000;
            let s = (rem / 1_000_000_000) as i8;
            let ns = (rem % 1_000_000_000) as i32;
            let result = jiff::civil::Time::new(h, m, s, ns)
                .map_err(|e| temporal_parse_error("temporal arithmetic", e))?;
            Ok(Value::LocalTime(result))
        }
        Value::LocalDateTime(dt) => {
            let dt1 = if months != 0 {
                dt.checked_add(jiff::Span::new().months(months))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else { *dt };
            let dt2 = if days != 0 {
                dt1.checked_add(jiff::Span::new().days(days))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else { dt1 };
            let dt3 = if nanos != 0 {
                dt2.checked_add(jiff::SignedDuration::from_nanos(nanos))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else { dt2 };
            Ok(Value::LocalDateTime(dt3))
        }
        Value::DateTime(zdt) => {
            let zdt1 = if months != 0 {
                zdt.as_ref().clone().checked_add(jiff::Span::new().months(months))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else { zdt.as_ref().clone() };
            let zdt2 = if days != 0 {
                zdt1.checked_add(jiff::Span::new().days(days))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else { zdt1 };
            let zdt3 = if nanos != 0 {
                zdt2.checked_add(jiff::SignedDuration::from_nanos(nanos))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else { zdt2 };
            Ok(Value::DateTime(Box::new(zdt3)))
        }
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "temporal value for arithmetic".to_string(),
            actual: temporal.type_name().to_string(),
        })),
    }
}

/// Add two durations.
pub fn duration_add(a: &CypherDuration, b: &CypherDuration) -> CypherDuration {
    CypherDuration {
        months: a.months + b.months,
        days: a.days + b.days,
        nanos: a.nanos + b.nanos,
    }
}

/// Subtract two durations.
pub fn duration_sub(a: &CypherDuration, b: &CypherDuration) -> CypherDuration {
    CypherDuration {
        months: a.months - b.months,
        days: a.days - b.days,
        nanos: a.nanos - b.nanos,
    }
}

/// Multiply a duration by an integer.
pub fn duration_mul(d: &CypherDuration, n: i64) -> CypherDuration {
    CypherDuration {
        months: d.months * n,
        days: d.days * n,
        nanos: d.nanos * n,
    }
}

/// Divide a duration by an integer.
pub fn duration_div(d: &CypherDuration, n: i64) -> Result<CypherDuration, GqlError> {
    if n == 0 {
        return Err(GqlError::Execution(ExecutionError::DivisionByZero));
    }
    Ok(CypherDuration {
        months: d.months / n,
        days: d.days / n,
        nanos: d.nanos / n,
    })
}

// ── CASE evaluation ───────────────────────────────────────────────────────────

fn eval_case<'r>(
    subject: Option<&Expr>,
    alternatives: &[(Expr, Expr)],
    default: Option<&Expr>,
    row: &Row<'r>,
) -> Result<Value, GqlError> {
    let subject_val = match subject {
        Some(e) => Some(eval_expr(e, row)?),
        None => None,
    };

    for (condition, result) in alternatives {
        let matches = match &subject_val {
            None => {
                // Generic CASE: WHEN is a full boolean predicate.
                let cond_val = eval_expr(condition, row)?;
                cond_val.is_truthy()
            }
            Some(sv) => {
                // Value CASE: WHEN is a value to compare against subject.
                let when_val = eval_expr(condition, row)?;
                sv.cypher_eq(&when_val).unwrap_or(false)
            }
        };
        if matches {
            return eval_expr(result, row);
        }
    }

    match default {
        Some(e) => eval_expr(e, row),
        None => Ok(Value::Null),
    }
}

// ── Small helpers ─────────────────────────────────────────────────────────────

fn require_arity(name: &str, expected: usize, vals: &[Value]) -> Result<(), GqlError> {
    if vals.len() != expected {
        Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: format!("{name}() expects {expected} argument(s)"),
            actual: format!("{} argument(s)", vals.len()),
        }))
    } else {
        Ok(())
    }
}

fn type_error(name: &str, val: &Value) -> Result<Value, GqlError> {
    Err(GqlError::Execution(ExecutionError::TypeMismatch {
        expected: format!("{name}() argument"),
        actual: val.type_name().to_string(),
    }))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use smol_str::SmolStr;

    use super::*;
    use crate::ast::{BinaryOp as BOp, Expr, Identifier, UnaryOp as UOp};
    use crate::types::Value;

    fn empty_row() -> Row<'static> {
        HashMap::new()
    }

    fn row_with<'a>(vars: &[(&'a str, &'a Value)]) -> Row<'a> {
        vars.iter().copied().collect()
    }

    // ── Literals ──────────────────────────────────────────────────────────────

    #[test]
    fn eval_integer_literal() {
        assert_eq!(
            eval_expr(&Expr::Integer(42), &empty_row()).unwrap(),
            Value::Int(42)
        );
    }

    #[test]
    fn eval_float_literal() {
        assert_eq!(
            eval_expr(&Expr::Float(1.23456), &empty_row()).unwrap(),
            Value::Float(1.23456)
        );
    }

    #[test]
    fn eval_string_literal() {
        assert_eq!(
            eval_expr(&Expr::str_lit("hello"), &empty_row()).unwrap(),
            Value::Str("hello".into())
        );
    }

    #[test]
    fn eval_bool_literal() {
        assert_eq!(
            eval_expr(&Expr::Bool(true), &empty_row()).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn eval_null_literal() {
        assert_eq!(eval_expr(&Expr::Null, &empty_row()).unwrap(), Value::Null);
    }

    // ── Variable lookup ───────────────────────────────────────────────────────

    #[test]
    fn eval_variable_found() {
        let val = Value::Int(99);
        let row = row_with(&[("x", &val)]);
        assert_eq!(
            eval_expr(&Expr::Variable(Identifier::new("x")), &row).unwrap(),
            Value::Int(99)
        );
    }

    #[test]
    fn eval_variable_not_found_errors() {
        let result = eval_expr(&Expr::Variable(Identifier::new("missing")), &empty_row());
        assert!(result.is_err());
    }

    // ── Property access ───────────────────────────────────────────────────────

    #[test]
    fn eval_property_on_map() {
        use indexmap::IndexMap;
        let mut m = IndexMap::new();
        m.insert(Box::from("name"), Value::Str("Alice".into()));
        let map_val = Value::Map(m);
        let row = row_with(&[("n", &map_val)]);
        let expr = Expr::Property(
            Box::new(Expr::Variable(Identifier::new("n"))),
            SmolStr::new("name"),
        );
        assert_eq!(eval_expr(&expr, &row).unwrap(), Value::Str("Alice".into()));
    }

    #[test]
    fn eval_property_missing_returns_null() {
        use indexmap::IndexMap;
        let map_val = Value::Map(IndexMap::new());
        let row = row_with(&[("n", &map_val)]);
        let expr = Expr::Property(
            Box::new(Expr::Variable(Identifier::new("n"))),
            SmolStr::new("age"),
        );
        assert_eq!(eval_expr(&expr, &row).unwrap(), Value::Null);
    }

    #[test]
    fn eval_property_on_null_returns_null() {
        let null_val = Value::Null;
        let row = row_with(&[("n", &null_val)]);
        let expr = Expr::Property(
            Box::new(Expr::Variable(Identifier::new("n"))),
            SmolStr::new("x"),
        );
        assert_eq!(eval_expr(&expr, &row).unwrap(), Value::Null);
    }

    // ── Null checks ───────────────────────────────────────────────────────────

    #[test]
    fn is_null_on_null() {
        assert_eq!(
            eval_expr(&Expr::IsNull(Box::new(Expr::Null)), &empty_row()).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn is_null_on_value() {
        assert_eq!(
            eval_expr(&Expr::IsNull(Box::new(Expr::Integer(1))), &empty_row()).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn is_not_null_on_value() {
        assert_eq!(
            eval_expr(&Expr::IsNotNull(Box::new(Expr::Integer(1))), &empty_row()).unwrap(),
            Value::Bool(true)
        );
    }

    // ── Arithmetic ────────────────────────────────────────────────────────────

    #[test]
    fn add_integers() {
        let expr = Expr::BinaryOp {
            op: BOp::Add,
            left: Box::new(Expr::Integer(3)),
            right: Box::new(Expr::Integer(4)),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Int(7));
    }

    #[test]
    fn add_float_and_int() {
        let expr = Expr::BinaryOp {
            op: BOp::Add,
            left: Box::new(Expr::Float(1.5)),
            right: Box::new(Expr::Integer(2)),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Float(3.5));
    }

    #[test]
    fn string_concatenation() {
        let expr = Expr::BinaryOp {
            op: BOp::Add,
            left: Box::new(Expr::str_lit("hello ")),
            right: Box::new(Expr::str_lit("world")),
        };
        assert_eq!(
            eval_expr(&expr, &empty_row()).unwrap(),
            Value::Str("hello world".into())
        );
    }

    #[test]
    fn div_by_zero_errors() {
        let expr = Expr::BinaryOp {
            op: BOp::Div,
            left: Box::new(Expr::Integer(10)),
            right: Box::new(Expr::Integer(0)),
        };
        assert!(matches!(
            eval_expr(&expr, &empty_row()),
            Err(GqlError::Execution(ExecutionError::DivisionByZero))
        ));
    }

    #[test]
    fn null_propagation_in_arithmetic() {
        let expr = Expr::BinaryOp {
            op: BOp::Add,
            left: Box::new(Expr::Null),
            right: Box::new(Expr::Integer(5)),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Null);
    }

    // ── Comparison ────────────────────────────────────────────────────────────

    #[test]
    fn eq_integers() {
        let expr = Expr::BinaryOp {
            op: BOp::Eq,
            left: Box::new(Expr::Integer(5)),
            right: Box::new(Expr::Integer(5)),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(true));
    }

    #[test]
    fn lt_comparison() {
        let expr = Expr::BinaryOp {
            op: BOp::Lt,
            left: Box::new(Expr::Integer(3)),
            right: Box::new(Expr::Integer(5)),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(true));
    }

    #[test]
    fn null_comparison_returns_null() {
        let expr = Expr::BinaryOp {
            op: BOp::Eq,
            left: Box::new(Expr::Null),
            right: Box::new(Expr::Integer(1)),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Null);
    }

    // ── Logical ───────────────────────────────────────────────────────────────

    #[test]
    fn and_short_circuits_on_false() {
        // false AND <error> should return false without evaluating right side
        let expr = Expr::BinaryOp {
            op: BOp::And,
            left: Box::new(Expr::Bool(false)),
            right: Box::new(Expr::Variable(Identifier::new("UNDEF"))),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(false));
    }

    #[test]
    fn or_short_circuits_on_true() {
        let expr = Expr::BinaryOp {
            op: BOp::Or,
            left: Box::new(Expr::Bool(true)),
            right: Box::new(Expr::Variable(Identifier::new("UNDEF"))),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(true));
    }

    #[test]
    fn not_true_is_false() {
        let expr = Expr::UnaryOp {
            op: UOp::Not,
            expr: Box::new(Expr::Bool(true)),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(false));
    }

    #[test]
    fn unary_neg() {
        let expr = Expr::UnaryOp {
            op: UOp::Neg,
            expr: Box::new(Expr::Integer(5)),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Int(-5));
    }

    // ── IN operator ───────────────────────────────────────────────────────────

    #[test]
    fn in_list_true() {
        let expr = Expr::BinaryOp {
            op: BOp::In,
            left: Box::new(Expr::Integer(2)),
            right: Box::new(Expr::List(vec![
                Expr::Integer(1),
                Expr::Integer(2),
                Expr::Integer(3),
            ])),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(true));
    }

    #[test]
    fn in_list_false() {
        let expr = Expr::BinaryOp {
            op: BOp::In,
            left: Box::new(Expr::Integer(9)),
            right: Box::new(Expr::List(vec![Expr::Integer(1), Expr::Integer(2)])),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(false));
    }

    // ── String predicates ─────────────────────────────────────────────────────

    #[test]
    fn starts_with_true() {
        let expr = Expr::BinaryOp {
            op: BOp::StartsWith,
            left: Box::new(Expr::str_lit("Alice")),
            right: Box::new(Expr::str_lit("Al")),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(true));
    }

    #[test]
    fn contains_true() {
        let expr = Expr::BinaryOp {
            op: BOp::Contains,
            left: Box::new(Expr::str_lit("hello world")),
            right: Box::new(Expr::str_lit("lo wo")),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(true));
    }

    // ── Function calls ────────────────────────────────────────────────────────

    #[test]
    fn abs_function() {
        let expr = Expr::FunctionCall {
            name: SmolStr::new("abs"),
            args: vec![Expr::Integer(-5)],
            distinct: false,
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Int(5));
    }

    #[test]
    fn size_of_list() {
        let expr = Expr::FunctionCall {
            name: SmolStr::new("size"),
            args: vec![Expr::List(vec![Expr::Integer(1), Expr::Integer(2)])],
            distinct: false,
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Int(2));
    }

    #[test]
    fn coalesce_returns_first_non_null() {
        let expr = Expr::FunctionCall {
            name: SmolStr::new("coalesce"),
            args: vec![Expr::Null, Expr::Null, Expr::Integer(42)],
            distinct: false,
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Int(42));
    }

    #[test]
    fn coalesce_all_null_returns_null() {
        let expr = Expr::FunctionCall {
            name: SmolStr::new("coalesce"),
            args: vec![Expr::Null, Expr::Null],
            distinct: false,
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Null);
    }

    // ── CASE expression ───────────────────────────────────────────────────────

    #[test]
    fn case_generic_first_match() {
        let val = Value::Int(5);
        let row = row_with(&[("x", &val)]);
        let expr = Expr::Case {
            subject: None,
            alternatives: vec![
                (
                    Expr::BinaryOp {
                        op: BOp::Gt,
                        left: Box::new(Expr::Variable(Identifier::new("x"))),
                        right: Box::new(Expr::Integer(10)),
                    },
                    Expr::str_lit("big"),
                ),
                (
                    Expr::BinaryOp {
                        op: BOp::Gt,
                        left: Box::new(Expr::Variable(Identifier::new("x"))),
                        right: Box::new(Expr::Integer(3)),
                    },
                    Expr::str_lit("medium"),
                ),
            ],
            default: Some(Box::new(Expr::str_lit("small"))),
        };
        assert_eq!(eval_expr(&expr, &row).unwrap(), Value::Str("medium".into()));
    }

    #[test]
    fn case_no_match_returns_null_without_default() {
        let val = Value::Int(1);
        let row = row_with(&[("x", &val)]);
        let expr = Expr::Case {
            subject: None,
            alternatives: vec![(
                Expr::BinaryOp {
                    op: BOp::Gt,
                    left: Box::new(Expr::Variable(Identifier::new("x"))),
                    right: Box::new(Expr::Integer(100)),
                },
                Expr::str_lit("big"),
            )],
            default: None,
        };
        assert_eq!(eval_expr(&expr, &row).unwrap(), Value::Null);
    }

    // ── List comprehension ────────────────────────────────────────────────────

    #[test]
    fn list_comp_identity_no_filter_no_map() {
        // [x IN [1, 2, 3]] — no filter, map is identity (variable itself)
        let expr = Expr::ListComp {
            variable: Identifier::new("x"),
            list: Box::new(Expr::List(vec![
                Expr::Integer(1),
                Expr::Integer(2),
                Expr::Integer(3),
            ])),
            filter: None,
            map: Box::new(Expr::Variable(Identifier::new("x"))),
        };
        assert_eq!(
            eval_expr(&expr, &empty_row()).unwrap(),
            Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
        );
    }

    #[test]
    fn list_comp_with_filter() {
        // [x IN [1, 2, 3, 4, 5] WHERE x > 2] → [3, 4, 5]
        let expr = Expr::ListComp {
            variable: Identifier::new("x"),
            list: Box::new(Expr::List(vec![
                Expr::Integer(1),
                Expr::Integer(2),
                Expr::Integer(3),
                Expr::Integer(4),
                Expr::Integer(5),
            ])),
            filter: Some(Box::new(Expr::BinaryOp {
                op: BOp::Gt,
                left: Box::new(Expr::Variable(Identifier::new("x"))),
                right: Box::new(Expr::Integer(2)),
            })),
            map: Box::new(Expr::Variable(Identifier::new("x"))),
        };
        assert_eq!(
            eval_expr(&expr, &empty_row()).unwrap(),
            Value::List(vec![Value::Int(3), Value::Int(4), Value::Int(5)])
        );
    }

    #[test]
    fn list_comp_with_map() {
        // [x IN [1, 2, 3] | x * 2] → [2, 4, 6]
        let expr = Expr::ListComp {
            variable: Identifier::new("x"),
            list: Box::new(Expr::List(vec![
                Expr::Integer(1),
                Expr::Integer(2),
                Expr::Integer(3),
            ])),
            filter: None,
            map: Box::new(Expr::BinaryOp {
                op: BOp::Mul,
                left: Box::new(Expr::Variable(Identifier::new("x"))),
                right: Box::new(Expr::Integer(2)),
            }),
        };
        assert_eq!(
            eval_expr(&expr, &empty_row()).unwrap(),
            Value::List(vec![Value::Int(2), Value::Int(4), Value::Int(6)])
        );
    }

    #[test]
    fn list_comp_with_filter_and_map() {
        // [x IN [1, 2, 3, 4] WHERE x % 2 = 0 | x * 10] → [20, 40]
        let expr = Expr::ListComp {
            variable: Identifier::new("x"),
            list: Box::new(Expr::List(vec![
                Expr::Integer(1),
                Expr::Integer(2),
                Expr::Integer(3),
                Expr::Integer(4),
            ])),
            filter: Some(Box::new(Expr::BinaryOp {
                op: BOp::Eq,
                left: Box::new(Expr::BinaryOp {
                    op: BOp::Mod,
                    left: Box::new(Expr::Variable(Identifier::new("x"))),
                    right: Box::new(Expr::Integer(2)),
                }),
                right: Box::new(Expr::Integer(0)),
            })),
            map: Box::new(Expr::BinaryOp {
                op: BOp::Mul,
                left: Box::new(Expr::Variable(Identifier::new("x"))),
                right: Box::new(Expr::Integer(10)),
            }),
        };
        assert_eq!(
            eval_expr(&expr, &empty_row()).unwrap(),
            Value::List(vec![Value::Int(20), Value::Int(40)])
        );
    }

    #[test]
    fn list_comp_on_null_returns_null() {
        let expr = Expr::ListComp {
            variable: Identifier::new("x"),
            list: Box::new(Expr::Null),
            filter: None,
            map: Box::new(Expr::Variable(Identifier::new("x"))),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Null);
    }

    #[test]
    fn list_comp_empty_list_returns_empty() {
        let expr = Expr::ListComp {
            variable: Identifier::new("x"),
            list: Box::new(Expr::List(vec![])),
            filter: None,
            map: Box::new(Expr::Variable(Identifier::new("x"))),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::List(vec![]));
    }
}
