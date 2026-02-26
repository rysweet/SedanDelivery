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
use crate::types::Value;

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
                    let actual = if i < 0 {
                        let offset = (-i) as usize;
                        if offset > len {
                            return Err(GqlError::Execution(ExecutionError::IndexOutOfBounds {
                                index: i,
                                len,
                            }));
                        }
                        len - offset
                    } else {
                        i as usize
                    };
                    if actual >= len {
                        return Err(GqlError::Execution(ExecutionError::IndexOutOfBounds {
                            index: i,
                            len,
                        }));
                    }
                    Ok(items[actual].clone())
                }
                (list, idx) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "List[Integer]".to_string(),
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
                    let start = match start_expr {
                        Some(e) => match eval_expr(e, row)? {
                            Value::Int(i) => i.max(0).min(len) as usize,
                            Value::Null => 0,
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
                            Value::Int(i) => i.max(0).min(len) as usize,
                            Value::Null => len as usize,
                            other => {
                                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                    expected: "Integer".to_string(),
                                    actual: other.type_name().to_string(),
                                }))
                            }
                        },
                        None => len as usize,
                    };
                    Ok(Value::List(items[start..end.min(items.len())].to_vec()))
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
            // Guard float zero: IEEE 754 would return Inf/NaN; Cypher spec requires an error.
            (Value::Float(a), Value::Float(b)) => {
                if b == 0.0 {
                    return Err(GqlError::Execution(ExecutionError::DivisionByZero));
                }
                Ok(Value::Float(a / b))
            }
            (Value::Int(a), Value::Float(b)) => {
                if b == 0.0 {
                    return Err(GqlError::Execution(ExecutionError::DivisionByZero));
                }
                Ok(Value::Float(a as f64 / b))
            }
            (Value::Float(a), Value::Int(b)) => {
                if b == 0 {
                    return Err(GqlError::Execution(ExecutionError::DivisionByZero));
                }
                Ok(Value::Float(a / b as f64))
            }
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
            (Value::Float(a), Value::Float(b)) => {
                if b == 0.0 {
                    return Err(GqlError::Execution(ExecutionError::DivisionByZero));
                }
                Ok(Value::Float(a % b))
            }
            (Value::Int(a), Value::Float(b)) => {
                if b == 0.0 {
                    return Err(GqlError::Execution(ExecutionError::DivisionByZero));
                }
                Ok(Value::Float(a as f64 % b))
            }
            (Value::Float(a), Value::Int(b)) => {
                if b == 0 {
                    return Err(GqlError::Execution(ExecutionError::DivisionByZero));
                }
                Ok(Value::Float(a % b as f64))
            }
            (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "%".to_string(),
                left: l.type_name().to_string(),
                right: r.type_name().to_string(),
            })),
        },

        BinaryOp::Pow => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Int(a), Value::Int(b)) => {
                if b < 0 {
                    Ok(Value::Float((a as f64).powi(b as i32)))
                } else {
                    Ok(Value::Int(a.wrapping_pow(b as u32)))
                }
            }
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a.powf(b))),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float((a as f64).powf(b))),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a.powi(b as i32))),
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
            (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "STARTS WITH".to_string(),
                left: l.type_name().to_string(),
                right: r.type_name().to_string(),
            })),
        },
        BinaryOp::EndsWith => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Str(s), Value::Str(suffix)) => Ok(Value::Bool(s.ends_with(suffix.as_ref()))),
            (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "ENDS WITH".to_string(),
                left: l.type_name().to_string(),
                right: r.type_name().to_string(),
            })),
        },
        BinaryOp::Contains => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Str(s), Value::Str(needle)) => Ok(Value::Bool(s.contains(needle.as_ref()))),
            (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "CONTAINS".to_string(),
                left: l.type_name().to_string(),
                right: r.type_name().to_string(),
            })),
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
                    // null IN [1,2,null] → null; null IN [1,2] → false
                    if items.iter().any(|i| i.is_null()) {
                        Ok(Value::Null)
                    } else {
                        Ok(Value::Bool(false))
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
    let ord = cypher_compare(lv, rv)?;
    let result = if also_eq {
        ord == target || ord == std::cmp::Ordering::Equal
    } else {
        ord == target
    };
    Ok(Value::Bool(result))
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
        (Value::Int(x), Value::Float(y)) => (*x as f64).partial_cmp(y).ok_or_else(|| {
            GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "comparable Float".to_string(),
                actual: "NaN".to_string(),
            })
        }),
        (Value::Float(x), Value::Int(y)) => x.partial_cmp(&(*y as f64)).ok_or_else(|| {
            GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "comparable Float".to_string(),
                actual: "NaN".to_string(),
            })
        }),
        (Value::Str(x), Value::Str(y)) => Ok(x.as_ref().cmp(y.as_ref())),
        (Value::Bool(x), Value::Bool(y)) => Ok(x.cmp(y)),
        // Temporal comparisons
        (Value::Date(x), Value::Date(y)) => Ok(x.cmp(y)),
        (Value::LocalTime(x), Value::LocalTime(y)) => Ok(x.cmp(y)),
        (Value::LocalDateTime(x), Value::LocalDateTime(y)) => Ok(x.cmp(y)),
        (Value::DateTime(x), Value::DateTime(y)) => Ok(x.cmp(y)),
        (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
            op: "</>".to_string(),
            left: l.type_name().to_string(),
            right: r.type_name().to_string(),
        })),
    }
}

// ── Built-in function evaluation ──────────────────────────────────────────────

fn eval_function<'r>(
    name: &str,
    args: &[Expr],
    _distinct: bool,
    row: &Row<'r>,
) -> Result<Value, GqlError> {
    // Evaluate all arguments eagerly.
    let mut vals: Vec<Value> = Vec::with_capacity(args.len());
    for a in args {
        vals.push(eval_expr(a, row)?);
    }

    match name.to_ascii_lowercase().as_str() {
        // ── Scalar functions ──────────────────────────────────────────────────
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
                v => Ok(Value::Str(v.to_string().into_boxed_str())),
            }
        }
        "tointeger" | "toint" => {
            require_arity("toInteger", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Int(n) => Ok(Value::Int(*n)),
                Value::Float(f) => Ok(Value::Int(*f as i64)),
                Value::Str(s) => s.parse::<i64>().map(Value::Int).map_err(|_| {
                    GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "integer string".to_string(),
                        actual: s.to_string(),
                    })
                }),
                other => type_error("toInteger", other),
            }
        }
        "tofloat" => {
            require_arity("toFloat", 1, &vals)?;
            match &vals[0] {
                Value::Null => Ok(Value::Null),
                Value::Float(f) => Ok(Value::Float(*f)),
                Value::Int(n) => Ok(Value::Float(*n as f64)),
                Value::Str(s) => s.parse::<f64>().map(Value::Float).map_err(|_| {
                    GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "float string".to_string(),
                        actual: s.to_string(),
                    })
                }),
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
                    m.keys().map(|k| Value::Str(k.clone())).collect(),
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
            "dayOfWeek" => Ok(Value::Int(d.weekday().to_monday_one_offset() as i64)),
            "dayOfYear" => Ok(Value::Int(d.day_of_year() as i64)),
            "quarter" => Ok(Value::Int(((d.month() as i64 - 1) / 3) + 1)),
            "ordinalDay" => Ok(Value::Int(d.day_of_year() as i64)),
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
            "dayOfWeek" => Ok(Value::Int(dt.date().weekday().to_monday_one_offset() as i64)),
            "dayOfYear" => Ok(Value::Int(dt.date().day_of_year() as i64)),
            "quarter" => Ok(Value::Int(((dt.month() as i64 - 1) / 3) + 1)),
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
            "timezone" => Ok(Value::Str(
                zdt.time_zone()
                    .iana_name()
                    .unwrap_or("UTC")
                    .to_string()
                    .into_boxed_str(),
            )),
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
            "dayOfWeek" => {
                let d = jiff::civil::Date::from(&**zdt);
                Ok(Value::Int(d.weekday().to_monday_one_offset() as i64))
            }
            "dayOfYear" => {
                let d = jiff::civil::Date::from(&**zdt);
                Ok(Value::Int(d.day_of_year() as i64))
            }
            "quarter" => Ok(Value::Int(((zdt.month() as i64 - 1) / 3) + 1)),
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
        Value::Duration(sp) => match key {
            "years" => Ok(Value::Int(sp.get_years() as i64)),
            "months" => Ok(Value::Int(sp.get_months() as i64)),
            "weeks" => Ok(Value::Int(sp.get_weeks() as i64)),
            "days" => Ok(Value::Int(sp.get_days() as i64)),
            "hours" => Ok(Value::Int(sp.get_hours() as i64)),
            "minutes" => Ok(Value::Int(sp.get_minutes())),
            "seconds" => Ok(Value::Int(sp.get_seconds())),
            "milliseconds" => Ok(Value::Int(sp.get_milliseconds())),
            "microseconds" => Ok(Value::Int(sp.get_microseconds())),
            "nanoseconds" => Ok(Value::Int(sp.get_nanoseconds())),
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
    // Try IANA name first
    if let Ok(tz) = jiff::tz::TimeZone::get(tz_str) {
        return Ok(tz);
    }
    // Try fixed offset: "Z", "+HH:MM", "-HH:MM", "+HHMM", "-HHMM"
    if tz_str == "Z" || tz_str == "UTC" {
        return Ok(jiff::tz::TimeZone::UTC);
    }
    if let Some(offset_str) = tz_str
        .strip_prefix('+')
        .or_else(|| tz_str.strip_prefix('-'))
    {
        let negative = tz_str.starts_with('-');
        let parts: Vec<&str> = offset_str.split(':').collect();
        let (hours, minutes) = if parts.len() == 2 {
            let h: i32 = parts[0]
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            let m: i32 = parts[1]
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            (h, m)
        } else if offset_str.len() == 4 {
            let h: i32 = offset_str[..2]
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            let m: i32 = offset_str[2..]
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            (h, m)
        } else if offset_str.len() == 2 {
            let h: i32 = offset_str
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            (h, 0)
        } else {
            return Err(format!("invalid timezone offset: {tz_str}"));
        };
        let total_seconds = (hours * 3600 + minutes * 60) * if negative { -1 } else { 1 };
        let offset = jiff::tz::Offset::from_seconds(total_seconds).map_err(|e| e.to_string())?;
        return Ok(jiff::tz::TimeZone::fixed(offset));
    }
    Err(format!("unknown timezone: {tz_str}"))
}

fn temporal_parse_error(func: &str, msg: impl std::fmt::Display) -> GqlError {
    GqlError::Execution(ExecutionError::TypeMismatch {
        expected: format!("valid {func}() argument"),
        actual: msg.to_string(),
    })
}

fn eval_date_constructor(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.is_empty() {
        // date() with no args → current date (use today)
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
            let d: jiff::civil::Date = s.parse().map_err(|e| temporal_parse_error("date", e))?;
            Ok(Value::Date(d))
        }
        Value::Map(m) => {
            // Check if this is a week-based date construction
            if m.contains_key("week") {
                let year = map_get_int(m, "year", 0)?;
                let week = map_get_int(m, "week", 1)?;
                let day_of_week = map_get_int(m, "dayOfWeek", 1)?; // 1=Monday, 7=Sunday

                // If 'date' key is present, use it as base for year and dayOfWeek
                let (base_year, base_dow) = if let Some(Value::Date(base_date)) = m.get("date") {
                    let iso_dow = base_date.weekday().to_monday_one_offset();
                    let y = if m.contains_key("year") {
                        year
                    } else {
                        base_date.year() as i64
                    };
                    let dow = if m.contains_key("dayOfWeek") {
                        day_of_week
                    } else {
                        iso_dow as i64
                    };
                    (y, dow)
                } else {
                    (year, day_of_week)
                };

                // Calculate ISO week date
                // ISO 8601: Week 1 contains the first Thursday of the year
                // January 4th is always in week 1
                let jan4 = jiff::civil::Date::new(base_year as i16, 1, 4)
                    .map_err(|e| temporal_parse_error("date", e))?;
                let jan4_dow = jan4.weekday().to_monday_one_offset() as i64; // 1=Mon, 7=Sun
                                                                             // Monday of week 1
                let week1_monday_ordinal = 4 - jan4_dow + 1; // ordinal of Monday of week 1
                let target_ordinal = week1_monday_ordinal + (week - 1) * 7 + (base_dow - 1);

                let d = jiff::civil::Date::new(base_year as i16, 1, 1)
                    .map_err(|e| temporal_parse_error("date", e))?;
                let d = d
                    .checked_add(jiff::Span::new().days(target_ordinal - 1))
                    .map_err(|e| temporal_parse_error("date", e))?;
                Ok(Value::Date(d))
            } else {
                let year = map_get_int(m, "year", 0)? as i16;
                let month = map_get_int(m, "month", 1)? as i8;
                let day = map_get_int(m, "day", 1)? as i8;
                let d = jiff::civil::Date::new(year, month, day)
                    .map_err(|e| temporal_parse_error("date", e))?;
                Ok(Value::Date(d))
            }
        }
        other => type_error("date", other),
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
            // Try parsing as time only, stripping timezone info if present
            let t: jiff::civil::Time = s.parse().map_err(|e| temporal_parse_error("time", e))?;
            Ok(Value::LocalTime(t))
        }
        Value::Map(m) => {
            let hour = map_get_int(m, "hour", 0)? as i8;
            let minute = map_get_int(m, "minute", 0)? as i8;
            let second = map_get_int(m, "second", 0)? as i8;
            let nanosecond = map_get_int(m, "nanosecond", 0)? as i32;
            let t = jiff::civil::Time::new(hour, minute, second, nanosecond)
                .map_err(|e| temporal_parse_error("time", e))?;
            Ok(Value::LocalTime(t))
        }
        other => type_error("time", other),
    }
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
        Value::Map(m) => {
            let hour = map_get_int(m, "hour", 0)? as i8;
            let minute = map_get_int(m, "minute", 0)? as i8;
            let second = map_get_int(m, "second", 0)? as i8;
            let nanosecond = map_get_int(m, "nanosecond", 0)? as i32;
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
            // Try parsing as a full zoned datetime first, fallback to timestamp
            let zoned: jiff::Zoned = s.parse().map_err(|e| temporal_parse_error("datetime", e))?;
            Ok(Value::DateTime(Box::new(zoned)))
        }
        Value::Map(m) => {
            let year = map_get_int(m, "year", 0)? as i16;
            let month = map_get_int(m, "month", 1)? as i8;
            let day = map_get_int(m, "day", 1)? as i8;
            let hour = map_get_int(m, "hour", 0)? as i8;
            let minute = map_get_int(m, "minute", 0)? as i8;
            let second = map_get_int(m, "second", 0)? as i8;
            let nanosecond = map_get_int(m, "nanosecond", 0)? as i32;
            let tz_name = map_get_str(m, "timezone");

            let dt = jiff::civil::DateTime::new(year, month, day, hour, minute, second, nanosecond)
                .map_err(|e| temporal_parse_error("datetime", e))?;

            let tz = if let Some(ref tz_str) = tz_name {
                parse_timezone(tz_str).map_err(|e| temporal_parse_error("datetime", e))?
            } else {
                jiff::tz::TimeZone::UTC
            };
            let zoned = dt
                .to_zoned(tz)
                .map_err(|e| temporal_parse_error("datetime", e))?;
            Ok(Value::DateTime(Box::new(zoned)))
        }
        other => type_error("datetime", other),
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
            let dt: jiff::civil::DateTime = s
                .parse()
                .map_err(|e| temporal_parse_error("localdatetime", e))?;
            Ok(Value::LocalDateTime(dt))
        }
        Value::Map(m) => {
            let year = map_get_int(m, "year", 0)? as i16;
            let month = map_get_int(m, "month", 1)? as i8;
            let day = map_get_int(m, "day", 1)? as i8;
            let hour = map_get_int(m, "hour", 0)? as i8;
            let minute = map_get_int(m, "minute", 0)? as i8;
            let second = map_get_int(m, "second", 0)? as i8;
            let nanosecond = map_get_int(m, "nanosecond", 0)? as i32;
            let dt = jiff::civil::DateTime::new(year, month, day, hour, minute, second, nanosecond)
                .map_err(|e| temporal_parse_error("localdatetime", e))?;
            Ok(Value::LocalDateTime(dt))
        }
        other => type_error("localdatetime", other),
    }
}

fn eval_duration_constructor(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.is_empty() {
        return Ok(Value::Duration(jiff::Span::new()));
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
            let sp: jiff::Span = s.parse().map_err(|e| temporal_parse_error("duration", e))?;
            Ok(Value::Duration(sp))
        }
        Value::Map(m) => {
            let mut sp = jiff::Span::new();
            if let Some(Value::Int(v)) = m.get("years") {
                sp = sp.years(*v);
            }
            if let Some(Value::Int(v)) = m.get("months") {
                sp = sp.months(*v);
            }
            if let Some(Value::Int(v)) = m.get("weeks") {
                sp = sp.weeks(*v);
            }
            if let Some(Value::Int(v)) = m.get("days") {
                sp = sp.days(*v);
            }
            if let Some(Value::Int(v)) = m.get("hours") {
                sp = sp.hours(*v);
            }
            if let Some(Value::Int(v)) = m.get("minutes") {
                sp = sp.minutes(*v);
            }
            if let Some(Value::Int(v)) = m.get("seconds") {
                sp = sp.seconds(*v);
            }
            if let Some(Value::Int(v)) = m.get("nanoseconds") {
                sp = sp.nanoseconds(*v);
            }
            // Also support singular forms
            if let Some(Value::Int(v)) = m.get("year") {
                sp = sp.years(*v);
            }
            if let Some(Value::Int(v)) = m.get("month") {
                sp = sp.months(*v);
            }
            if let Some(Value::Int(v)) = m.get("week") {
                sp = sp.weeks(*v);
            }
            if let Some(Value::Int(v)) = m.get("day") {
                sp = sp.days(*v);
            }
            if let Some(Value::Int(v)) = m.get("hour") {
                sp = sp.hours(*v);
            }
            if let Some(Value::Int(v)) = m.get("minute") {
                sp = sp.minutes(*v);
            }
            if let Some(Value::Int(v)) = m.get("second") {
                sp = sp.seconds(*v);
            }
            if let Some(Value::Int(v)) = m.get("nanosecond") {
                sp = sp.nanoseconds(*v);
            }
            Ok(Value::Duration(sp))
        }
        other => type_error("duration", other),
    }
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
