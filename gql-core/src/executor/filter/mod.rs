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

mod builtins;
mod collections;
mod geo;
pub(crate) mod operators;
mod temporal;

use std::cell::RefCell;
use std::collections::HashMap;

use indexmap::IndexMap;

use crate::ast::{Expr, Identifier};
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

// ── Re-exports for external consumers ─────────────────────────────────────────

pub(crate) use self::collections::quantifier_result;
pub(crate) use self::operators::cypher_compare;
pub(crate) use self::temporal::time_to_utc_nanos;
pub use self::temporal::{
    duration_add, duration_div, duration_mul, duration_mul_f64, duration_sub,
    temporal_add_duration, temporal_property_access,
};

/// Hard cap on list sizes produced by range() and split() to prevent memory exhaustion DoS.
/// Reads from the deployment-time config (env `GQL_MAX_LIST_SIZE`, default 10 million).
#[inline]
fn max_list_size() -> usize {
    crate::config::global().max_list_size
}

// ── Query-level "now" for stable temporal function results ──────────────────────

thread_local! {
    /// The current query's "now" timestamp. Initialized once per query execution so that
    /// temporal no-arg functions (date(), time(), etc.) return the same value throughout.
    static QUERY_NOW: RefCell<Option<jiff::Zoned>> = const { RefCell::new(None) };

    /// Per-thread cache of compiled regexes to avoid recompilation on repeated pattern use.
    /// Uses `IndexMap` to preserve insertion order so we can evict the oldest half
    /// instead of clearing the entire cache when the capacity limit is reached.
    static REGEX_CACHE: RefCell<IndexMap<String, regex::Regex>> = RefCell::new(IndexMap::new());
}

/// Initialize the query-level "now". Call once at the start of each query execution.
pub fn init_query_now() {
    QUERY_NOW.with(|c| *c.borrow_mut() = Some(jiff::Zoned::now()));
}

/// Clear the query-level "now". Call at the end of each query execution.
pub fn clear_query_now() {
    QUERY_NOW.with(|c| *c.borrow_mut() = None);
}

/// RAII guard that initializes the query-level "now" on construction and clears
/// it on drop, ensuring `clear_query_now()` is always called even on error paths.
pub struct QueryTimeGuard;

impl QueryTimeGuard {
    /// Initialize the query "now" and return a guard that will clear it on drop.
    pub fn new() -> Self {
        init_query_now();
        QueryTimeGuard
    }
}

impl Default for QueryTimeGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for QueryTimeGuard {
    fn drop(&mut self) {
        clear_query_now();
    }
}

/// Get the query-level "now", falling back to the system time if not initialized.
fn get_query_now() -> jiff::Zoned {
    QUERY_NOW.with(|c| c.borrow().clone().unwrap_or_else(jiff::Zoned::now))
}

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
                | Value::Time(_, _)
                | Value::LocalDateTime(_)
                | Value::DateTime(_)
                | Value::Duration(_) => temporal_property_access(&obj, key.as_str()),
                // Point property access (x, y, z, srid, crs)
                Value::Point(_) => temporal_property_access(&obj, key.as_str()),
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

// ── Built-in function evaluation ──────────────────────────────────────────────

fn eval_function<'r>(
    name: &str,
    args: &[Expr],
    _distinct: bool,
    row: &Row<'r>,
) -> Result<Value, GqlError> {
    // Quantifier predicates need access to the raw ListComp expr —
    // they must NOT eagerly evaluate args (which would lose predicate info).
    match name.to_ascii_lowercase().as_str() {
        "all" | "any" | "none" | "single" => return collections::eval_quantifier(name, args, row),
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
        // ── Math functions ────────────────────────────────────────────────────
        "rand" => builtins::builtin_rand(vals),
        "abs" => builtins::builtin_abs(vals),
        "ceil" => builtins::builtin_ceil(vals),
        "floor" => builtins::builtin_floor(vals),
        "round" => builtins::builtin_round(vals),
        "sqrt" => builtins::builtin_sqrt(vals),
        "sign" => builtins::builtin_sign(vals),
        "log" => builtins::builtin_log(vals),
        "log10" => builtins::builtin_log10(vals),
        "exp" => builtins::builtin_exp(vals),
        "sin" => builtins::builtin_sin(vals),
        "cos" => builtins::builtin_cos(vals),
        "tan" => builtins::builtin_tan(vals),
        "asin" => builtins::builtin_asin(vals),
        "acos" => builtins::builtin_acos(vals),
        "atan" => builtins::builtin_atan(vals),
        "atan2" => builtins::builtin_atan2(vals),
        "degrees" => builtins::builtin_degrees(vals),
        "radians" => builtins::builtin_radians(vals),
        "haversin" => builtins::builtin_haversin(vals),
        "pi" => builtins::builtin_pi(vals),
        "e" => builtins::builtin_e(vals),

        // ── Type conversion functions ─────────────────────────────────────────
        "tostring" => builtins::builtin_tostring(vals),
        "tointeger" | "toint" => builtins::builtin_tointeger(vals),
        "tofloat" => builtins::builtin_tofloat(vals),
        "toboolean" => builtins::builtin_toboolean(vals),

        // ── String functions ──────────────────────────────────────────────────
        "size" => builtins::builtin_size(vals),
        "length" => builtins::builtin_length(vals),
        "toupper" => builtins::builtin_toupper(vals),
        "tolower" => builtins::builtin_tolower(vals),
        "trim" => builtins::builtin_trim(vals),
        "ltrim" => builtins::builtin_ltrim(vals),
        "rtrim" => builtins::builtin_rtrim(vals),
        "left" => builtins::builtin_left(vals),
        "right" => builtins::builtin_right(vals),
        "substring" => builtins::builtin_substring(vals),
        "replace" => builtins::builtin_replace(vals),
        "split" => builtins::builtin_split(vals),

        // ── List / collection functions ───────────────────────────────────────
        "head" => builtins::builtin_head(vals),
        "last" => builtins::builtin_last(vals),
        "tail" => builtins::builtin_tail(vals),
        "reverse" => builtins::builtin_reverse(vals),
        "coalesce" => builtins::builtin_coalesce(vals),
        "isnull" => builtins::builtin_isnull(vals),
        "keys" => builtins::builtin_keys(vals),
        "values" => builtins::builtin_values(vals),
        "properties" => builtins::builtin_properties(vals),
        "range" => builtins::builtin_range(vals),

        // ── Temporal constructor functions ───────────────────────────────────
        "date" => temporal::eval_date_constructor(&vals),
        "time" => temporal::eval_time_constructor(&vals),
        "localtime" => temporal::eval_localtime_constructor(&vals),
        "datetime" => temporal::eval_datetime_constructor(&vals),
        "localdatetime" => temporal::eval_localdatetime_constructor(&vals),
        "duration" => temporal::eval_duration_constructor(&vals),
        "point" => geo::eval_point_constructor(&vals),

        // ── Temporal namespaced functions ─────────────────────────────────────
        "date.truncate" => temporal::eval_date_truncate(&vals),
        "datetime.truncate" => temporal::eval_datetime_truncate(&vals),
        "localdatetime.truncate" => temporal::eval_localdatetime_truncate(&vals),
        "localtime.truncate" => temporal::eval_localtime_truncate(&vals),
        "time.truncate" => temporal::eval_time_truncate(&vals),

        "date.realtime" | "date.statement" | "date.transaction" => {
            temporal::builtin_temporal_realtime_date(vals)
        }
        "datetime.realtime" | "datetime.statement" | "datetime.transaction" => {
            temporal::builtin_temporal_realtime_datetime(vals)
        }
        "localdatetime.realtime" | "localdatetime.statement" | "localdatetime.transaction" => {
            temporal::builtin_temporal_realtime_localdatetime(vals)
        }
        "localtime.realtime" | "localtime.statement" | "localtime.transaction" => {
            temporal::builtin_temporal_realtime_localtime(vals)
        }
        "time.realtime" | "time.statement" | "time.transaction" => {
            temporal::builtin_temporal_realtime_time(vals)
        }
        "datetime.fromepoch" => temporal::builtin_datetime_fromepoch(vals),
        "datetime.fromepochmillis" => temporal::builtin_datetime_fromepochmillis(vals),

        "duration.between" => temporal::eval_duration_between(&vals),
        "duration.inmonths" => temporal::eval_duration_in_months(&vals),
        "duration.indays" => temporal::eval_duration_in_days(&vals),
        "duration.inseconds" => temporal::eval_duration_in_seconds(&vals),

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

        // ── Gremlin string/type/collection functions ──────────────────────────
        "gremlin_tolower" => match vals.first() {
            Some(Value::Str(s)) => Ok(Value::Str(s.to_lowercase().into())),
            Some(Value::Null) => Ok(Value::Null),
            Some(Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "String".into(), actual: "List".into(),
            })),
            _ => Ok(Value::Null),
        },
        "gremlin_toupper" => match vals.first() {
            Some(Value::Str(s)) => Ok(Value::Str(s.to_uppercase().into())),
            Some(Value::Null) => Ok(Value::Null),
            Some(Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "String".into(), actual: "List".into(),
            })),
            _ => Ok(Value::Null),
        },
        "gremlin_trim" => match vals.first() {
            Some(Value::Str(s)) => Ok(Value::Str(s.trim().into())),
            Some(Value::Null) => Ok(Value::Null),
            Some(Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "String".into(), actual: "List".into(),
            })),
            _ => Ok(Value::Null),
        },
        "gremlin_ltrim" => match vals.first() {
            Some(Value::Str(s)) => Ok(Value::Str(s.trim_start().into())),
            Some(Value::Null) => Ok(Value::Null),
            Some(Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "String".into(), actual: "List".into(),
            })),
            _ => Ok(Value::Null),
        },
        "gremlin_rtrim" => match vals.first() {
            Some(Value::Str(s)) => Ok(Value::Str(s.trim_end().into())),
            Some(Value::Null) => Ok(Value::Null),
            Some(Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "String".into(), actual: "List".into(),
            })),
            _ => Ok(Value::Null),
        },
        "gremlin_length" => match vals.first() {
            Some(Value::Str(s)) => Ok(Value::Int(s.len() as i64)),
            Some(Value::Null) => Ok(Value::Null),
            Some(other) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "The length() step can only take string as argument".into(),
                actual: other.type_name().into(),
            })),
            _ => Ok(Value::Int(0)),
        },
        "gremlin_reverse" => match vals.first() {
            Some(Value::Str(s)) => Ok(Value::Str(s.chars().rev().collect::<String>().into())),
            Some(Value::List(l)) => {
                let mut rev = l.clone();
                rev.reverse();
                Ok(Value::List(rev))
            }
            Some(Value::Null) => Ok(Value::Null),
            _ => Ok(Value::Null),
        },
        "gremlin_replace" => match (vals.first(), vals.get(1), vals.get(2)) {
            (Some(Value::Str(s)), Some(Value::Str(old)), Some(Value::Str(new))) => {
                Ok(Value::Str(s.replace(old.as_ref(), new.as_ref()).into()))
            }
            (Some(Value::Null), _, _) => Ok(Value::Null),
            (Some(Value::List(_)), _, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "String".into(), actual: "List".into(),
            })),
            _ => Ok(Value::Null),
        },
        "gremlin_split" => match (vals.first(), vals.get(1)) {
            (Some(Value::Str(s)), Some(Value::Str(sep))) => {
                let parts: Vec<Value> = if sep.is_empty() {
                    // Empty separator: split into individual characters
                    s.chars().map(|c| Value::Str(c.to_string().into())).collect()
                } else {
                    s.split(sep.as_ref())
                        .map(|p| Value::Str(p.into()))
                        .collect()
                };
                Ok(Value::List(parts))
            }
            (Some(Value::Null), _) => Ok(Value::Null),
            (Some(Value::List(_)), _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "String".into(), actual: "List".into(),
            })),
            _ => Ok(Value::Null),
        },
        "gremlin_substring" => match vals.first() {
            Some(Value::Str(s)) => {
                let chars: Vec<char> = s.chars().collect();
                let len = chars.len() as i64;
                let raw_start = match vals.get(1) {
                    Some(Value::Int(n)) => *n,
                    _ => 0,
                };
                // Resolve negative start: negative means from end
                let start = if raw_start < 0 {
                    (len + raw_start).max(0) as usize
                } else {
                    (raw_start as usize).min(chars.len())
                };
                if start >= chars.len() {
                    return Ok(Value::Str("".into()));
                }
                let result = match vals.get(2) {
                    Some(Value::Int(raw_end)) => {
                        // Gremlin: second arg is END INDEX (not length)
                        let re = *raw_end;
                        if re < 0 {
                            // Negative end: end = len + raw_end
                            let end = ((len + re) as usize).min(chars.len());
                            if end <= start {
                                String::new()
                            } else {
                                chars[start..end].iter().collect::<String>()
                            }
                        } else {
                            let end = (re as usize).min(chars.len());
                            if end <= start {
                                String::new()
                            } else {
                                chars[start..end].iter().collect::<String>()
                            }
                        }
                    }
                    _ => chars[start..].iter().collect::<String>(),
                };
                Ok(Value::Str(result.into()))
            }
            Some(Value::Null) => Ok(Value::Null),
            Some(Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "The substring() step can only take string as argument".into(),
                actual: "list".into(),
            })),
            _ => Ok(Value::Null),
        },
        "gremlin_substring_local" => {
            // Apply substring to each element in a list
            let list = match vals.first() {
                Some(Value::List(items)) => items,
                _ => return Ok(Value::Null),
            };
            let start = match vals.get(1) {
                Some(Value::Int(n)) => *n,
                _ => 0,
            };
            let end = match vals.get(2) {
                Some(Value::Int(n)) => Some(*n),
                _ => None,
            };
            let results: Vec<Value> = list
                .iter()
                .map(|v| {
                    if let Value::Str(s) = v {
                        let chars: Vec<char> = s.chars().collect();
                        let len = chars.len() as i64;
                        let s_idx = if start < 0 {
                            (len + start).max(0) as usize
                        } else {
                            (start as usize).min(chars.len())
                        };
                        let result = match end {
                            Some(e) => {
                                let e_idx = if e < 0 {
                                    ((len + e) as usize).min(chars.len())
                                } else {
                                    (e as usize).min(chars.len())
                                };
                                if e_idx <= s_idx {
                                    String::new()
                                } else {
                                    chars[s_idx..e_idx].iter().collect()
                                }
                            }
                            None => chars[s_idx..].iter().collect(),
                        };
                        Value::Str(result.into())
                    } else {
                        v.clone()
                    }
                })
                .collect();
            Ok(Value::List(results))
        }
        "gremlin_tolower_local" => match vals.first() {
            Some(Value::List(items)) => {
                let results: Vec<Value> = items.iter().map(|v| match v {
                    Value::Str(s) => Value::Str(s.to_lowercase().into()),
                    Value::Null => Value::Null,
                    other => other.clone(),
                }).collect();
                Ok(Value::List(results))
            }
            _ => eval_function_with_vals("gremlin_tolower", vals),
        },
        "gremlin_toupper_local" => match vals.first() {
            Some(Value::List(items)) => {
                let results: Vec<Value> = items.iter().map(|v| match v {
                    Value::Str(s) => Value::Str(s.to_uppercase().into()),
                    Value::Null => Value::Null,
                    other => other.clone(),
                }).collect();
                Ok(Value::List(results))
            }
            _ => eval_function_with_vals("gremlin_toupper", vals),
        },
        "gremlin_trim_local" => match vals.first() {
            Some(Value::List(items)) => {
                // Validate all elements are String or Null
                for v in items {
                    if !matches!(v, Value::Str(_) | Value::Null) {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "The trim(local) step can only take string or list of strings".into(),
                            actual: v.type_name().into(),
                        }));
                    }
                }
                let results: Vec<Value> = items.iter().map(|v| match v {
                    Value::Str(s) => Value::Str(s.trim().into()),
                    _ => Value::Null,
                }).collect();
                Ok(Value::List(results))
            }
            _ => eval_function_with_vals("gremlin_trim", vals),
        },
        "gremlin_ltrim_local" => match vals.first() {
            Some(Value::List(items)) => {
                for v in items {
                    if !matches!(v, Value::Str(_) | Value::Null) {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "The lTrim(local) step can only take string or list of strings".into(),
                            actual: v.type_name().into(),
                        }));
                    }
                }
                let results: Vec<Value> = items.iter().map(|v| match v {
                    Value::Str(s) => Value::Str(s.trim_start().into()),
                    _ => Value::Null,
                }).collect();
                Ok(Value::List(results))
            }
            _ => eval_function_with_vals("gremlin_ltrim", vals),
        },
        "gremlin_rtrim_local" => match vals.first() {
            Some(Value::List(items)) => {
                for v in items {
                    if !matches!(v, Value::Str(_) | Value::Null) {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "The rTrim(local) step can only take string or list of strings".into(),
                            actual: v.type_name().into(),
                        }));
                    }
                }
                let results: Vec<Value> = items.iter().map(|v| match v {
                    Value::Str(s) => Value::Str(s.trim_end().into()),
                    _ => Value::Null,
                }).collect();
                Ok(Value::List(results))
            }
            _ => eval_function_with_vals("gremlin_rtrim", vals),
        },
        "gremlin_replace_local" => match (vals.first(), vals.get(1), vals.get(2)) {
            (Some(Value::List(items)), Some(Value::Str(old)), Some(Value::Str(new))) => {
                let results: Vec<Value> = items.iter().map(|v| match v {
                    Value::Str(s) => Value::Str(s.replace(old.as_ref(), new.as_ref()).into()),
                    Value::Null => Value::Null,
                    other => other.clone(),
                }).collect();
                Ok(Value::List(results))
            }
            _ => eval_function_with_vals("gremlin_replace", vals),
        },
        "gremlin_split_local" => match (vals.first(), vals.get(1)) {
            (Some(Value::List(items)), Some(Value::Str(sep))) => {
                let results: Vec<Value> = items.iter().map(|v| match v {
                    Value::Str(s) => {
                        let parts: Vec<Value> = if sep.is_empty() {
                            s.chars().map(|c| Value::Str(c.to_string().into())).collect()
                        } else {
                            s.split(sep.as_ref())
                                .map(|p| Value::Str(p.into()))
                                .collect()
                        };
                        Value::List(parts)
                    }
                    Value::Null => Value::Null,
                    other => other.clone(),
                }).collect();
                Ok(Value::List(results))
            }
            _ => eval_function_with_vals("gremlin_split", vals),
        },
        "gremlin_length_local" => match vals.first() {
            Some(Value::List(l)) => Ok(Value::Int(l.len() as i64)),
            Some(Value::Map(m)) => Ok(Value::Int(m.len() as i64)),
            Some(Value::Str(s)) => Ok(Value::Int(s.len() as i64)),
            Some(Value::Null) => Ok(Value::Null),
            _ => Ok(Value::Int(0)),
        },
        "gremlin_math" => {
            let current_val = vals.first().cloned().unwrap_or(Value::Null);
            let expr_str = match vals.get(1) {
                Some(Value::Str(s)) => s.as_ref(),
                _ => return Ok(Value::Null),
            };
            eval_math_expr(expr_str, &current_val)
        },
        "gremlin_tail_local" => {
            let val = vals.first().cloned().unwrap_or(Value::Null);
            let n = match vals.get(1) {
                Some(Value::Int(i)) => *i as usize,
                _ => 1,
            };
            match val {
                Value::List(items) => {
                    let start = items.len().saturating_sub(n);
                    Ok(Value::List(items[start..].to_vec()))
                }
                Value::Map(m) => {
                    let entries: Vec<_> = m.iter().collect();
                    let start = entries.len().saturating_sub(n);
                    let result: IndexMap<Box<str>, Value> = entries[start..]
                        .iter()
                        .map(|(k, v)| ((*k).clone(), (*v).clone()))
                        .collect();
                    Ok(Value::Map(result))
                }
                Value::Null => Ok(Value::Null),
                other => Ok(other),
            }
        }
        "gremlin_concat" => {
            // Gremlin concat: first arg is current traverser, rest are args to append.
            // Null traverser (first arg) with no other args → null.
            // Null args → skipped.
            // List as traverser (first arg) → error.
            if vals.is_empty() {
                return Ok(Value::Null);
            }
            if matches!(vals.first(), Some(Value::List(_))) {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "String concat() can only take string as argument".into(),
                    actual: "list".into(),
                }));
            }
            // If only arg is null traverser and no concat args, return null
            if vals.len() == 1 && matches!(vals.first(), Some(Value::Null)) {
                return Ok(Value::Null);
            }
            let mut result = String::new();
            for v in vals.iter() {
                match v {
                    Value::Str(s) => result.push_str(s),
                    Value::Int(n) => result.push_str(&n.to_string()),
                    Value::Float(f) => result.push_str(&f.to_string()),
                    Value::Bool(b) => result.push_str(&b.to_string()),
                    Value::Null => {
                        // Null args are skipped
                    }
                    _ => result.push_str(&format!("{:?}", v)),
                }
            }
            Ok(Value::Str(result.into()))
        }
        "gremlin_conjoin" => match (vals.first(), vals.get(1)) {
            (Some(Value::List(items)), Some(Value::Str(sep))) => {
                let parts: Vec<String> = items
                    .iter()
                    .filter_map(|v| match v {
                        Value::Str(s) => Some(s.to_string()),
                        Value::Int(n) => Some(n.to_string()),
                        Value::Float(f) => Some(f.to_string()),
                        Value::Bool(b) => Some(b.to_string()),
                        Value::Null => None, // skip null elements
                        other => Some(format!("{:?}", other)),
                    })
                    .collect();
                Ok(Value::Str(parts.join(sep.as_ref()).into()))
            }
            (Some(Value::Null), _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Incoming traverser for conjoin step can't be null".into(),
                actual: "null".into(),
            })),
            (_, Some(Value::Str(_))) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "conjoin step can only take an array or an Iterable type for incoming traversers, encountered".into(),
                actual: "non-list".into(),
            })),
            _ => Ok(Value::Null),
        },
        "gremlin_format" => {
            // args: [traverser, pattern, optional_labels_map]
            match (vals.first(), vals.get(1)) {
                (Some(val), Some(Value::Str(pattern))) => {
                    let mut result = pattern.to_string();
                    // First, resolve %{label} from optional labels map (3rd arg)
                    if let Some(Value::Map(labels)) = vals.get(2) {
                        for (k, v) in labels.iter() {
                            let placeholder = format!("%{{{}}}", k);
                            if result.contains(&placeholder) {
                                let val_str = match v {
                                    Value::Str(s) => s.to_string(),
                                    Value::Int(n) => n.to_string(),
                                    Value::Float(f) => f.to_string(),
                                    Value::Bool(b) => b.to_string(),
                                    _ => format!("{:?}", v),
                                };
                                result = result.replace(&placeholder, &val_str);
                            }
                        }
                    }
                    // Then resolve %{key} from the traverser value if it's a Map
                    if result.contains("%{") {
                        if let Value::Map(m) = val {
                            for (k, v) in m.iter() {
                                let placeholder = format!("%{{{}}}", k);
                                let val_str = match v {
                                    Value::Str(s) => s.to_string(),
                                    Value::Int(n) => n.to_string(),
                                    Value::Float(f) => f.to_string(),
                                    Value::Bool(b) => b.to_string(),
                                    _ => format!("{:?}", v),
                                };
                                result = result.replace(&placeholder, &val_str);
                            }
                        }
                    }
                    // Replace %s with the traverser value directly
                    if result.contains("%s") {
                        let val_str = match val {
                            Value::Str(s) => s.to_string(),
                            Value::Int(n) => n.to_string(),
                            Value::Float(f) => f.to_string(),
                            _ => String::new(),
                        };
                        result = result.replace("%s", &val_str);
                    }
                    Ok(Value::Str(result.into()))
                }
                (Some(Value::Null), _) => Ok(Value::Null),
                _ => Ok(Value::Null),
            }
        }
        "gremlin_format_by" => {
            // format() with by() modulations: args = [traverser, pattern, by1, by2, ...]
            // Replace %{_} placeholders cyclically with by-values.
            // Also resolve %{name} from node/edge properties.
            if vals.len() < 2 {
                return Ok(Value::Null);
            }
            if matches!(vals.first(), Some(Value::Null)) {
                return Ok(Value::Null);
            }
            let pattern = match vals.get(1) {
                Some(Value::Str(s)) => s.to_string(),
                _ => return Ok(Value::Null),
            };
            let by_values = &vals[2..];
            let mut result = pattern;
            // Replace %{_} with by-values cyclically
            let mut by_idx = 0;
            while let Some(pos) = result.find("%{_}") {
                if by_values.is_empty() {
                    break;
                }
                let val = &by_values[by_idx % by_values.len()];
                let val_str = match val {
                    Value::Str(s) => s.to_string(),
                    Value::Int(n) => n.to_string(),
                    Value::Float(f) => f.to_string(),
                    Value::Bool(b) => b.to_string(),
                    Value::Null => return Ok(Value::Null),
                    _ => format!("{:?}", val),
                };
                result = format!("{}{}{}", &result[..pos], val_str, &result[pos + 4..]);
                by_idx += 1;
            }
            // If there are still %{key} placeholders, try resolving from node/edge properties
            if result.contains("%{") {
                return Ok(Value::Null);
            }
            Ok(Value::Str(result.into()))
        }
        "gremlin_asstring" => match vals.first() {
            Some(Value::Str(s)) => Ok(Value::Str(s.clone())),
            Some(Value::Int(n)) => Ok(Value::Str(n.to_string().into())),
            Some(Value::Float(f)) => {
                if f.is_nan() {
                    Ok(Value::Str("NaN".into()))
                } else if f.is_infinite() {
                    Ok(Value::Str(if *f > 0.0 { "Infinity" } else { "-Infinity" }.into()))
                } else {
                    Ok(Value::Str(f.to_string().into()))
                }
            }
            Some(Value::Bool(b)) => Ok(Value::Str(b.to_string().into())),
            Some(Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Can't parse null as string".into(),
                actual: "null".into(),
            })),
            Some(Value::List(items)) => {
                let parts: Vec<String> = items.iter().map(|v| match v {
                    Value::Str(s) => s.to_string(),
                    Value::Int(n) => n.to_string(),
                    Value::Float(f) => f.to_string(),
                    Value::Bool(b) => b.to_string(),
                    Value::Null => "null".to_string(),
                    other => format!("{:?}", other),
                }).collect();
                Ok(Value::Str(format!("[{}]", parts.join(", ")).into()))
            }
            Some(Value::Map(m)) => {
                let parts: Vec<String> = m.iter().map(|(k, v)| {
                    let vs = match v {
                        Value::Str(s) => s.to_string(),
                        Value::Int(n) => n.to_string(),
                        Value::Float(f) => f.to_string(),
                        Value::Bool(b) => b.to_string(),
                        Value::List(items) => {
                            let inner: Vec<String> = items.iter().map(|i| match i {
                                Value::Str(s) => s.to_string(),
                                Value::Int(n) => n.to_string(),
                                other => format!("{:?}", other),
                            }).collect();
                            format!("[{}]", inner.join(", "))
                        }
                        other => format!("{:?}", other),
                    };
                    format!("{}={}", k, vs)
                }).collect();
                Ok(Value::Str(format!("{{{}}}", parts.join(", ")).into()))
            }
            Some(Value::Node(nid)) => Ok(Value::Str(format!("v[{}]", nid.0).into())),
            Some(Value::Edge(eid)) => Ok(Value::Str(format!("e[{}]", eid.0).into())),
            Some(Value::Path { nodes, edges }) => {
                let parts: Vec<String> = nodes.iter().map(|n| format!("v[{}]", n.0))
                    .chain(edges.iter().map(|e| format!("e[{}]", e.0)))
                    .collect();
                Ok(Value::Str(format!("path[{}]", parts.join(", ")).into()))
            }
            Some(other) => Ok(Value::Str(format!("{:?}", other).into())),
            None => Ok(Value::Null),
        },
        "gremlin_asstring_local" => {
            // asString(Scope.local): convert list elements to strings
            fn value_to_str(v: &Value) -> Result<Value, GqlError> {
                match v {
                    Value::Str(s) => Ok(Value::Str(s.clone())),
                    Value::Int(n) => Ok(Value::Str(n.to_string().into())),
                    Value::Float(f) => Ok(Value::Str(f.to_string().into())),
                    Value::Bool(b) => Ok(Value::Str(b.to_string().into())),
                    Value::Node(nid) => Ok(Value::Str(format!("v[{}]", nid.0).into())),
                    Value::Edge(eid) => Ok(Value::Str(format!("e[{}]", eid.0).into())),
                    Value::Null => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "Can't parse null as string".into(),
                        actual: "null".into(),
                    })),
                    other => Ok(Value::Str(format!("{:?}", other).into())),
                }
            }
            match vals.first() {
                Some(Value::List(items)) => {
                    let converted: Result<Vec<Value>, GqlError> =
                        items.iter().map(value_to_str).collect();
                    Ok(Value::List(converted?))
                }
                Some(other) => value_to_str(other),
                None => Ok(Value::Null),
            }
        }
        "gremlin_asnumber" => match vals.first() {
            Some(Value::Int(n)) => Ok(Value::Int(*n)),
            Some(Value::Float(f)) => Ok(Value::Float(*f)),
            Some(Value::Str(s)) => {
                // Try hex first: 0x5
                if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                    if let Ok(n) = i64::from_str_radix(hex, 16) {
                        return Ok(Value::Int(n));
                    }
                }
                if let Ok(n) = s.parse::<i64>() {
                    Ok(Value::Int(n))
                } else if let Ok(f) = s.parse::<f64>() {
                    Ok(Value::Float(f))
                } else {
                    Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: format!("Can't parse string '{}' as number.", s),
                        actual: format!("'{}'", s),
                    }))
                }
            }
            Some(Value::Bool(b)) => Ok(Value::Int(if *b { 1 } else { 0 })),
            Some(Value::Null) => Ok(Value::Null),
            Some(Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Can't parse type ArrayList as number.".into(),
                actual: "list".into(),
            })),
            _ => Ok(Value::Null),
        },
        "gremlin_asnumber_typed" => {
            // asNumber(GType.INT/LONG/FLOAT/DOUBLE): convert to specific numeric type
            match (vals.first(), vals.get(1)) {
                (Some(Value::Float(f)), Some(Value::Str(t))) if t.as_ref() == "int" => {
                    Ok(Value::Int(*f as i64))
                }
                (Some(Value::Int(n)), Some(Value::Str(t))) if t.as_ref() == "float" => {
                    Ok(Value::Float(*n as f64))
                }
                (Some(Value::Str(s)), Some(Value::Str(t))) if t.as_ref() == "int" => {
                    if let Ok(n) = s.parse::<i64>() {
                        Ok(Value::Int(n))
                    } else if let Ok(f) = s.parse::<f64>() {
                        Ok(Value::Int(f as i64))
                    } else {
                        Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: format!("Can't parse string '{}' as integer.", s),
                            actual: format!("'{}'", s),
                        }))
                    }
                }
                (Some(Value::Str(s)), Some(Value::Str(t))) if t.as_ref() == "float" => {
                    if let Ok(f) = s.parse::<f64>() {
                        Ok(Value::Float(f))
                    } else {
                        Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: format!("Can't parse string '{}' as float.", s),
                            actual: format!("'{}'", s),
                        }))
                    }
                }
                (Some(val), _) => {
                    // Fall through to default asNumber behavior
                    match val {
                        Value::Int(n) => Ok(Value::Int(*n)),
                        Value::Float(f) => Ok(Value::Float(*f)),
                        Value::Null => Ok(Value::Null),
                        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "Cannot convert to number".into(),
                            actual: val.type_name().to_string(),
                        })),
                    }
                }
                _ => Ok(Value::Null),
            }
        }
        "gremlin_asbool" => match vals.first() {
            Some(Value::Bool(b)) => Ok(Value::Bool(*b)),
            Some(Value::Int(n)) => Ok(Value::Bool(*n != 0)),
            Some(Value::Float(f)) => {
                if f.is_nan() {
                    Ok(Value::Bool(false))
                } else {
                    Ok(Value::Bool(*f != 0.0))
                }
            }
            Some(Value::Str(s)) => match s.to_lowercase().as_str() {
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "boolean-compatible value".into(),
                    actual: format!("'{}'", s),
                })),
            },
            Some(Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "boolean-compatible value".into(),
                actual: "null".into(),
            })),
            Some(Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "boolean-compatible value".into(),
                actual: "list".into(),
            })),
            _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "boolean-compatible value".into(),
                actual: "unknown".into(),
            })),
        },
        "gremlin_typeof" => match vals.first() {
            Some(Value::Int(_)) => Ok(Value::Str("integer".into())),
            Some(Value::Float(_)) => Ok(Value::Str("double".into())),
            Some(Value::Str(_)) => Ok(Value::Str("string".into())),
            Some(Value::Bool(_)) => Ok(Value::Str("boolean".into())),
            Some(Value::Null) => Ok(Value::Str("null".into())),
            Some(Value::List(_)) => Ok(Value::Str("list".into())),
            Some(Value::Map(_)) => Ok(Value::Str("map".into())),
            Some(Value::Node(_)) => Ok(Value::Str("vertex".into())),
            Some(Value::Edge(_)) => Ok(Value::Str("edge".into())),
            Some(Value::Date(_)) => Ok(Value::Str("date".into())),
            Some(Value::Path { .. }) => Ok(Value::Str("path".into())),
            _ => Ok(Value::Str("unknown".into())),
        },
        "gremlin_combine" => {
            let lhs = vals.first().cloned().unwrap_or(Value::Null);
            let rhs = vals.get(1).cloned().unwrap_or(Value::Null);
            match (&lhs, &rhs) {
                (Value::Null, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Incoming traverser for combine step can't be null".into(),
                    actual: "null".into(),
                })),
                (Value::List(_), Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for combine step must yield an iterable type, not null".into(),
                    actual: "null".into(),
                })),
                (Value::List(l), Value::List(r)) => {
                    let mut result = l.clone();
                    result.extend(r.iter().cloned());
                    Ok(Value::List(result))
                }
                (_, Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "combine step can only take an array or an Iterable type for incoming traversers, encountered".into(),
                    actual: format!("{:?}", lhs),
                })),
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for combine step must yield an iterable type, encountered".into(),
                    actual: format!("{:?}", rhs),
                })),
            }
        }
        "gremlin_merge" => {
            let lhs = vals.first().cloned().unwrap_or(Value::Null);
            let rhs = vals.get(1).cloned().unwrap_or(Value::Null);
            match (&lhs, &rhs) {
                (Value::Null, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Incoming traverser for merge step can't be null".into(),
                    actual: "null".into(),
                })),
                (Value::List(_), Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for merge step must yield an iterable type, not null".into(),
                    actual: "null".into(),
                })),
                (Value::List(l), Value::List(r)) => {
                    // merge() is set-union: unique elements from both lists
                    let mut result = l.clone();
                    for item in r {
                        if !result.iter().any(|existing| existing == item) {
                            result.push(item.clone());
                        }
                    }
                    Ok(Value::List(result))
                }
                (Value::Map(l), Value::Map(r)) => {
                    // Map merge: combine maps
                    let mut result = l.clone();
                    for (k, v) in r.iter() {
                        result.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                    Ok(Value::Map(result))
                }
                (_, Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "merge step can only take an array or an Iterable type for incoming traversers, encountered".into(),
                    actual: format!("{:?}", lhs),
                })),
                (_, Value::Map(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "merge step expected provided argument to evaluate to a Map, encountered".into(),
                    actual: format!("{:?}", rhs),
                })),
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for merge step must yield an iterable type, encountered".into(),
                    actual: format!("{:?}", rhs),
                })),
            }
        }
        "gremlin_intersect" => {
            let lhs = vals.first().cloned().unwrap_or(Value::Null);
            let rhs = vals.get(1).cloned().unwrap_or(Value::Null);
            match (&lhs, &rhs) {
                (Value::Null, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Incoming traverser for intersect step can't be null".into(),
                    actual: "null".into(),
                })),
                (Value::List(_), Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for intersect step must yield an iterable type, not null".into(),
                    actual: "null".into(),
                })),
                (Value::List(l), Value::List(r)) => {
                    let mut result = Vec::new();
                    let mut rhs_remaining: Vec<&Value> = r.iter().collect();
                    for item in l {
                        if let Some(pos) = rhs_remaining.iter().position(|rv| *rv == item) {
                            result.push(item.clone());
                            rhs_remaining.remove(pos);
                        }
                    }
                    Ok(Value::List(result))
                }
                (_, Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "intersect step can only take an array or an Iterable type for incoming traversers, encountered".into(),
                    actual: format!("{:?}", lhs),
                })),
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for intersect step must yield an iterable type, encountered".into(),
                    actual: format!("{:?}", rhs),
                })),
            }
        }
        "gremlin_difference" => {
            let lhs = vals.first().cloned().unwrap_or(Value::Null);
            let rhs = vals.get(1).cloned().unwrap_or(Value::Null);
            match (&lhs, &rhs) {
                (Value::Null, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Incoming traverser for difference step can't be null".into(),
                    actual: "null".into(),
                })),
                (Value::List(_), Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for difference step must yield an iterable type, not null".into(),
                    actual: "null".into(),
                })),
                (Value::List(l), Value::List(r)) => {
                    let mut result = Vec::new();
                    let mut rhs_remaining: Vec<&Value> = r.iter().collect();
                    for item in l {
                        if let Some(pos) = rhs_remaining.iter().position(|rv| *rv == item) {
                            rhs_remaining.remove(pos);
                        } else {
                            result.push(item.clone());
                        }
                    }
                    Ok(Value::List(result))
                }
                (_, Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "difference step can only take an array or an Iterable type for incoming traversers".into(),
                    actual: format!("{:?}", lhs),
                })),
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for difference step must yield an iterable type, encountered".into(),
                    actual: format!("{:?}", rhs),
                })),
            }
        }
        "gremlin_disjunct" => {
            let lhs = vals.first().cloned().unwrap_or(Value::Null);
            let rhs = vals.get(1).cloned().unwrap_or(Value::Null);
            match (&lhs, &rhs) {
                (Value::Null, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Incoming traverser for disjunct step can't be null".into(),
                    actual: "null".into(),
                })),
                (Value::List(_), Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for disjunct step must yield an iterable type, not null".into(),
                    actual: "null".into(),
                })),
                (Value::List(l), Value::List(r)) => {
                    // Symmetric difference: items in l not in r, plus items in r not in l
                    let mut result = Vec::new();
                    let mut r_remaining: Vec<&Value> = r.iter().collect();
                    for item in l {
                        if let Some(pos) = r_remaining.iter().position(|rv| *rv == item) {
                            r_remaining.remove(pos);
                        } else {
                            result.push(item.clone());
                        }
                    }
                    for item in r_remaining {
                        result.push(item.clone());
                    }
                    Ok(Value::List(result))
                }
                (_, Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "disjunct step can only take an array or an Iterable type for incoming traversers, encountered".into(),
                    actual: format!("{:?}", lhs),
                })),
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for disjunct step must yield an iterable type, encountered".into(),
                    actual: format!("{:?}", rhs),
                })),
            }
        }
        "gremlin_product" => {
            let lhs = vals.first().cloned().unwrap_or(Value::Null);
            let rhs = vals.get(1).cloned().unwrap_or(Value::Null);
            match (&lhs, &rhs) {
                (Value::Null, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Incoming traverser for product step can't be null".into(),
                    actual: "null".into(),
                })),
                (Value::List(_), Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for product step must yield an iterable type, not null".into(),
                    actual: "null".into(),
                })),
                (Value::List(l), Value::List(r)) => {
                    if l.is_empty() || r.is_empty() {
                        return Ok(Value::List(Vec::new()));
                    }
                    let mut result = Vec::new();
                    for a in l {
                        for b in r {
                            result.push(Value::List(vec![a.clone(), b.clone()]));
                        }
                    }
                    Ok(Value::List(result))
                }
                (_, Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "product step can only take an array or an Iterable type for incoming traversers, encountered".into(),
                    actual: format!("{:?}", lhs),
                })),
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "traversal argument for product step must yield an iterable type, encountered".into(),
                    actual: format!("{:?}", rhs),
                })),
            }
        }
        "gremlin_all" => {
            // all(pred): filter step — returns the list if ALL elements match, else filters out
            let list_val = vals.first().cloned().unwrap_or(Value::Null);
            match &list_val {
                Value::List(items) => {
                    // pred info in vals[1] (op string) and vals[2] (value)
                    let pred_op = match vals.get(1) {
                        Some(Value::Str(s)) => s.as_ref(),
                        _ => "neq",
                    };
                    let pred_val = vals.get(2).cloned().unwrap_or(Value::Null);
                    let all_match = items.iter().all(|v| gremlin_pred_check(v, pred_op, &pred_val));
                    Ok(Value::Bool(all_match))
                }
                _ => Ok(Value::Bool(false)),
            }
        }
        "gremlin_any" => {
            let list_val = vals.first().cloned().unwrap_or(Value::Null);
            match &list_val {
                Value::List(items) => {
                    let pred_op = match vals.get(1) {
                        Some(Value::Str(s)) => s.as_ref(),
                        _ => "neq",
                    };
                    let pred_val = vals.get(2).cloned().unwrap_or(Value::Null);
                    let any_match = items.iter().any(|v| gremlin_pred_check(v, pred_op, &pred_val));
                    Ok(Value::Bool(any_match))
                }
                _ => Ok(Value::Bool(false)),
            }
        }
        "gremlin_none" => {
            let list_val = vals.first().cloned().unwrap_or(Value::Null);
            let pred_op = match vals.get(1) {
                Some(Value::Str(s)) => s.as_ref(),
                _ => "neq",
            };
            let pred_val = vals.get(2).cloned().unwrap_or(Value::Null);
            match &list_val {
                Value::List(items) => {
                    let none_match = !items.iter().any(|v| gremlin_pred_check(v, pred_op, &pred_val));
                    Ok(Value::Bool(none_match))
                }
                // Scalar: none(P) → NOT matches(P)
                other => Ok(Value::Bool(!gremlin_pred_check(other, pred_op, &pred_val))),
            }
        }

        "gremlin_wraplist" => {
            // Wrap a single value in a list for valueMap
            match vals.into_iter().next() {
                Some(v) => Ok(Value::List(vec![v])),
                None => Ok(Value::List(Vec::new())),
            }
        }
        "gremlin_valuemap" => {
            // Full valueMap: extract all properties and wrap in lists
            match vals.into_iter().next() {
                Some(Value::Map(m)) => {
                    let wrapped: IndexMap<Box<str>, Value> = m
                        .into_iter()
                        .map(|(k, v)| (k, Value::List(vec![v])))
                        .collect();
                    Ok(Value::Map(wrapped))
                }
                Some(other) => Ok(other),
                None => Ok(Value::Null),
            }
        }
        "list_compact" => {
            let result: Vec<Value> = vals
                .into_iter()
                .filter(|v| !matches!(v, Value::Null))
                .collect();
            Ok(Value::List(result))
        }
        "gremlin_values_all" => {
            // Fallback for non-graph values (maps, etc.)
            match vals.into_iter().next() {
                Some(Value::Map(m)) => {
                    // Property Map from properties(): extract ~value
                    if m.contains_key("~value") {
                        let val = m.get("~value").cloned().unwrap_or(Value::Null);
                        Ok(Value::List(vec![val]))
                    } else {
                        let vals: Vec<Value> = m.into_values().collect();
                        Ok(Value::List(vals))
                    }
                }
                Some(Value::Null) | None => Ok(Value::List(vec![])),
                Some(other) => Ok(Value::List(vec![other])),
            }
        }
        "gremlin_sort_local" => {
            let mut vals_iter = vals.into_iter();
            let list_val = vals_iter.next().unwrap_or(Value::Null);
            let by_key = match vals_iter.next() {
                Some(Value::Str(s)) if !s.is_empty() => Some(s),
                _ => None,
            };
            let descending =
                matches!(vals_iter.next(), Some(Value::Str(s)) if s.as_ref() == "desc");

            match list_val {
                Value::List(mut items) => {
                    if let Some(ref key) = by_key {
                        // Sort by property key — extract key from each item for comparison
                        items.sort_by(|a, b| {
                            let a_prop = extract_sort_key(a, key);
                            let b_prop = extract_sort_key(b, key);
                            let ord = crate::executor::engine::helpers::gremlin_cmp_values(
                                &a_prop, &b_prop,
                            );
                            if descending { ord.reverse() } else { ord }
                        });
                    } else {
                        items.sort_by(|a, b| {
                            let ord =
                                crate::executor::engine::helpers::gremlin_cmp_values(a, b);
                            if descending { ord.reverse() } else { ord }
                        });
                    }
                    Ok(Value::List(items))
                }
                Value::Map(m) => {
                    // Sort map entries by value (Column.values) or key (Column.keys)
                    let mut entries: Vec<(Box<str>, Value)> = m.into_iter().collect();
                    if let Some(ref key) = by_key {
                        if key.as_ref() == "Column.keys" {
                            entries.sort_by(|(ka, _), (kb, _)| ka.cmp(kb));
                        } else {
                            entries.sort_by(|(_, va), (_, vb)| {
                                crate::executor::engine::helpers::gremlin_cmp_values(va, vb)
                            });
                        }
                    } else {
                        entries.sort_by(|(ka, _), (kb, _)| ka.cmp(kb));
                    }
                    if descending {
                        entries.reverse();
                    }
                    let result: indexmap::IndexMap<Box<str>, Value> =
                        entries.into_iter().collect();
                    Ok(Value::Map(result))
                }
                other => Ok(other),
            }
        }
        "gremlin_sum_local" => match vals.first() {
            Some(Value::List(items)) => {
                if items.is_empty() {
                    return Ok(Value::Null);
                }
                let mut sum_i: i64 = 0;
                let mut sum_f: f64 = 0.0;
                let mut is_float = false;
                let mut has_value = false;
                for v in items {
                    match v {
                        Value::Int(n) => { sum_i += n; has_value = true; }
                        Value::Float(f) => { sum_f += f; is_float = true; has_value = true; }
                        Value::Null => {} // skip nulls
                        _ => {}
                    }
                }
                if !has_value {
                    Ok(Value::Null)
                } else if is_float {
                    Ok(Value::Float(sum_f + sum_i as f64))
                } else {
                    Ok(Value::Int(sum_i))
                }
            }
            Some(Value::Int(n)) => Ok(Value::Int(*n)),
            Some(Value::Float(f)) => Ok(Value::Float(*f)),
            Some(Value::Null) => Ok(Value::Null),
            _ => Ok(Value::Null),
        },
        "gremlin_min_local" => match vals.first() {
            Some(Value::List(items)) => {
                let mut result: Option<Value> = None;
                for v in items {
                    if matches!(v, Value::Null) { continue; }
                    result = Some(match result {
                        None => v.clone(),
                        Some(cur) => {
                            if let Ok(std::cmp::Ordering::Less) = super::filter::cypher_compare(v, &cur) {
                                v.clone()
                            } else {
                                cur
                            }
                        }
                    });
                }
                Ok(result.unwrap_or(Value::Null))
            }
            Some(Value::Int(n)) => Ok(Value::Int(*n)),
            Some(Value::Float(f)) => Ok(Value::Float(*f)),
            Some(Value::Str(s)) => Ok(Value::Str(s.clone())),
            Some(Value::Null) => Ok(Value::Null),
            _ => Ok(Value::Null),
        },
        "gremlin_max_local" => match vals.first() {
            Some(Value::List(items)) => {
                let mut result: Option<Value> = None;
                for v in items {
                    if matches!(v, Value::Null) { continue; }
                    result = Some(match result {
                        None => v.clone(),
                        Some(cur) => {
                            if let Ok(std::cmp::Ordering::Greater) = super::filter::cypher_compare(v, &cur) {
                                v.clone()
                            } else {
                                cur
                            }
                        }
                    });
                }
                Ok(result.unwrap_or(Value::Null))
            }
            Some(Value::Int(n)) => Ok(Value::Int(*n)),
            Some(Value::Float(f)) => Ok(Value::Float(*f)),
            Some(Value::Str(s)) => Ok(Value::Str(s.clone())),
            Some(Value::Null) => Ok(Value::Null),
            _ => Ok(Value::Null),
        },
        "gremlin_mean_local" => match vals.first() {
            Some(Value::List(items)) => {
                let mut sum: f64 = 0.0;
                let mut count: usize = 0;
                for v in items {
                    match v {
                        Value::Int(n) => { sum += *n as f64; count += 1; }
                        Value::Float(f) => { sum += f; count += 1; }
                        _ => {}
                    }
                }
                if count == 0 {
                    Ok(Value::Null)
                } else {
                    Ok(Value::Float(sum / count as f64))
                }
            }
            Some(Value::Int(n)) => Ok(Value::Float(*n as f64)),
            Some(Value::Float(f)) => Ok(Value::Float(*f)),
            Some(Value::Null) => Ok(Value::Null),
            _ => Ok(Value::Null),
        },

        "gremlin_containing" => match (vals.first(), vals.get(1)) {
            (Some(Value::Str(haystack)), Some(Value::Str(needle))) => {
                Ok(Value::Bool(haystack.contains(needle.as_ref())))
            }
            (Some(Value::Null), _) | (_, Some(Value::Null)) => Ok(Value::Null),
            _ => Ok(Value::Bool(false)),
        },
        "gremlin_startingwith" => match (vals.first(), vals.get(1)) {
            (Some(Value::Str(haystack)), Some(Value::Str(prefix))) => {
                Ok(Value::Bool(haystack.starts_with(prefix.as_ref())))
            }
            (Some(Value::Null), _) | (_, Some(Value::Null)) => Ok(Value::Null),
            _ => Ok(Value::Bool(false)),
        },
        "gremlin_endingwith" => match (vals.first(), vals.get(1)) {
            (Some(Value::Str(haystack)), Some(Value::Str(suffix))) => {
                Ok(Value::Bool(haystack.ends_with(suffix.as_ref())))
            }
            (Some(Value::Null), _) | (_, Some(Value::Null)) => Ok(Value::Null),
            _ => Ok(Value::Bool(false)),
        },
        "gremlin_regex" => match (vals.first(), vals.get(1)) {
            (Some(Value::Str(haystack)), Some(Value::Str(pattern))) => {
                match regex::Regex::new(pattern.as_ref()) {
                    Ok(re) => Ok(Value::Bool(re.is_match(haystack))),
                    Err(_) => Ok(Value::Bool(false)),
                }
            }
            (Some(Value::Null), _) | (_, Some(Value::Null)) => Ok(Value::Null),
            _ => Ok(Value::Bool(false)),
        },
        "gremlin_istype" => {
            match (vals.first(), vals.get(1)) {
                (Some(val), Some(Value::Str(expected_type))) => {
                    let actual_type = match val {
                        Value::Int(_) => "integer",
                        Value::Float(_) => "double",
                        Value::Str(_) => "string",
                        Value::Bool(_) => "boolean",
                        Value::Null => "null",
                        Value::List(_) => "list",
                        Value::Map(_) => "map",
                        Value::Node(_) => "vertex",
                        Value::Edge(_) => "edge",
                        Value::Date(_) => "date",
                        Value::Path { .. } => "path",
                        _ => "unknown",
                    };
                    // Handle various type name formats
                    let expected = expected_type.to_lowercase();
                    let matches = actual_type == expected
                        || (expected == "long" && actual_type == "integer")
                        || (expected == "short" && actual_type == "integer")
                        || (expected == "byte" && actual_type == "integer")
                        || (expected == "biginteger" && actual_type == "integer")
                        || (expected == "float" && actual_type == "double")
                        || (expected == "bigdecimal" && actual_type == "double")
                        || (expected == "set" && actual_type == "list");
                    Ok(Value::Bool(matches))
                }
                _ => Ok(Value::Bool(false)),
            }
        }

        // ── Unknown function ──────────────────────────────────────────────────
        other => Err(GqlError::Execution(ExecutionError::unsupported(format!(
            "unknown function '{other}'"
        )))),
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

// ── Gremlin predicate helpers ─────────────────────────────────────────────────

/// Evaluate a Gremlin predicate check for all()/any()/none() steps.
fn gremlin_pred_check(val: &Value, pred_op: &str, pred_val: &Value) -> bool {
    match pred_op {
        "eq" => gremlin_eq(val, pred_val),
        "neq" => !gremlin_eq(val, pred_val),
        "gt" => gremlin_cmp(val, pred_val) == Some(std::cmp::Ordering::Greater),
        "gte" => matches!(
            gremlin_cmp(val, pred_val),
            Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        ),
        "lt" => gremlin_cmp(val, pred_val) == Some(std::cmp::Ordering::Less),
        "lte" => matches!(
            gremlin_cmp(val, pred_val),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        ),
        "within" => {
            if let Value::List(options) = pred_val {
                options.iter().any(|o| gremlin_eq(val, o))
            } else {
                false
            }
        }
        "without" => {
            if let Value::List(options) = pred_val {
                !options.iter().any(|o| gremlin_eq(val, o))
            } else {
                true
            }
        }
        _ => false,
    }
}

/// Gremlin equality: null == null is true, NaN != NaN, Int/Float coercion.
fn gremlin_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Null, _) | (_, Value::Null) => false,
        (Value::Float(f), _) if f.is_nan() => false,
        (_, Value::Float(f)) if f.is_nan() => false,
        (Value::Int(i), Value::Float(f)) | (Value::Float(f), Value::Int(i)) => {
            let fi = *i as f64;
            fi as i64 == *i && fi == *f
        }
        _ => a == b,
    }
}

/// Gremlin ordering for comparable values.
fn gremlin_cmp(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Null, Value::Null) => Some(std::cmp::Ordering::Equal),
        (Value::Null, _) | (_, Value::Null) => None,
        (Value::Float(f), _) if f.is_nan() => None,
        (_, Value::Float(f)) if f.is_nan() => None,
        (Value::Int(i1), Value::Int(i2)) => i1.partial_cmp(i2),
        (Value::Float(f1), Value::Float(f2)) => f1.partial_cmp(f2),
        (Value::Int(i), Value::Float(f)) => (*i as f64).partial_cmp(f),
        (Value::Float(f), Value::Int(i)) => f.partial_cmp(&(*i as f64)),
        (Value::Str(s1), Value::Str(s2)) => Some(s1.cmp(s2)),
        (Value::Bool(b1), Value::Bool(b2)) => b1.partial_cmp(b2),
        _ => None,
    }
}

/// Extract a sort key from a value (node, edge, or map) by property name.
fn extract_sort_key(v: &Value, key: &str) -> Value {
    match v {
        Value::Map(m) => m.get(key).cloned().unwrap_or(Value::Null),
        // For nodes/edges, we can't directly access properties here
        // since we don't have GraphStore access in filter context.
        _ => Value::Null,
    }
}

/// Evaluate a Gremlin math expression. `_` refers to the current value.
/// Supports: `_ + N`, `_ - N`, `_ * N`, `_ / N`, `ceil(_)`, `floor(_)`,
/// `abs(_)`, `_ + 1` (inc).
fn eval_math_expr(expr: &str, current: &Value) -> Result<Value, GqlError> {
    let expr = expr.trim();

    // Helper to extract numeric value
    fn as_f64(v: &Value) -> Option<f64> {
        match v {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    let val = as_f64(current).unwrap_or(0.0);

    // ceil(_), floor(_), abs(_)
    if expr.starts_with("ceil(") {
        return Ok(Value::Float(val.ceil()));
    }
    if expr.starts_with("floor(") {
        return Ok(Value::Float(val.floor()));
    }
    if expr.starts_with("abs(") {
        return Ok(Value::Float(val.abs()));
    }
    if expr.starts_with("sqrt(") {
        return Ok(Value::Float(val.sqrt()));
    }

    // Binary operations: _ op N
    for (op_str, op_fn) in &[
        (" + ", (|a: f64, b: f64| a + b) as fn(f64, f64) -> f64),
        (" - ", |a, b| a - b),
        (" * ", |a, b| a * b),
        (" / ", |a, b| if b != 0.0 { a / b } else { f64::NAN }),
        (" % ", |a, b| if b != 0.0 { a % b } else { f64::NAN }),
    ] {
        if let Some(pos) = expr.find(op_str) {
            let left_str = expr[..pos].trim();
            let right_str = expr[pos + op_str.len()..].trim();

            let left_val = if left_str == "_" {
                val
            } else if let Ok(n) = left_str.parse::<f64>() {
                n
            } else {
                val // default to current value for variable refs
            };
            let right_val = if right_str == "_" {
                val
            } else if let Ok(n) = right_str.parse::<f64>() {
                n
            } else {
                val
            };

            let result = op_fn(left_val, right_val);
            // Return Int if the result is a whole number and both operands are int-like
            if result.fract() == 0.0 && matches!(current, Value::Int(_)) {
                return Ok(Value::Float(result));
            }
            return Ok(Value::Float(result));
        }
    }

    // Fallback: return current value unchanged
    Ok(current.clone())
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

    // ── Built-in scalar functions ─────────────────────────────────────────────

    #[test]
    fn builtin_ceil_float() {
        assert_eq!(
            eval_function_with_vals("ceil", vec![Value::Float(2.3)]).unwrap(),
            Value::Float(3.0)
        );
    }

    #[test]
    fn builtin_ceil_int_unchanged() {
        assert_eq!(
            eval_function_with_vals("ceil", vec![Value::Int(5)]).unwrap(),
            Value::Int(5)
        );
    }

    #[test]
    fn builtin_floor_float() {
        assert_eq!(
            eval_function_with_vals("floor", vec![Value::Float(3.9)]).unwrap(),
            Value::Float(3.0)
        );
    }

    #[test]
    fn builtin_round_float() {
        assert_eq!(
            eval_function_with_vals("round", vec![Value::Float(2.5)]).unwrap(),
            Value::Float(3.0)
        );
    }

    #[test]
    fn builtin_sqrt_int() {
        assert_eq!(
            eval_function_with_vals("sqrt", vec![Value::Int(9)]).unwrap(),
            Value::Float(3.0)
        );
    }

    #[test]
    fn builtin_sign_positive() {
        assert_eq!(
            eval_function_with_vals("sign", vec![Value::Int(5)]).unwrap(),
            Value::Int(1)
        );
    }

    #[test]
    fn builtin_sign_negative() {
        assert_eq!(
            eval_function_with_vals("sign", vec![Value::Int(-3)]).unwrap(),
            Value::Int(-1)
        );
    }

    #[test]
    fn builtin_tostring_int() {
        assert_eq!(
            eval_function_with_vals("toString", vec![Value::Int(42)]).unwrap(),
            Value::Str("42".into())
        );
    }

    #[test]
    fn builtin_tostring_bool() {
        assert_eq!(
            eval_function_with_vals("toString", vec![Value::Bool(true)]).unwrap(),
            Value::Str("true".into())
        );
    }

    #[test]
    fn builtin_tointeger_from_string() {
        assert_eq!(
            eval_function_with_vals("toInteger", vec![Value::Str("42".into())]).unwrap(),
            Value::Int(42)
        );
    }

    #[test]
    fn builtin_tointeger_from_float() {
        assert_eq!(
            eval_function_with_vals("toInteger", vec![Value::Float(3.9)]).unwrap(),
            Value::Int(3)
        );
    }

    #[test]
    fn builtin_tofloat_from_int() {
        assert_eq!(
            eval_function_with_vals("toFloat", vec![Value::Int(5)]).unwrap(),
            Value::Float(5.0)
        );
    }

    #[test]
    fn builtin_tofloat_invalid_string_returns_null() {
        assert_eq!(
            eval_function_with_vals("toFloat", vec![Value::Str("abc".into())]).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn builtin_toboolean_true_string() {
        assert_eq!(
            eval_function_with_vals("toBoolean", vec![Value::Str("true".into())]).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn builtin_toboolean_invalid_returns_null() {
        assert_eq!(
            eval_function_with_vals("toBoolean", vec![Value::Str("yes".into())]).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn builtin_tolower() {
        assert_eq!(
            eval_function_with_vals("toLower", vec![Value::Str("HELLO".into())]).unwrap(),
            Value::Str("hello".into())
        );
    }

    #[test]
    fn builtin_toupper() {
        assert_eq!(
            eval_function_with_vals("toUpper", vec![Value::Str("hello".into())]).unwrap(),
            Value::Str("HELLO".into())
        );
    }

    #[test]
    fn builtin_trim() {
        assert_eq!(
            eval_function_with_vals("trim", vec![Value::Str("  hello  ".into())]).unwrap(),
            Value::Str("hello".into())
        );
    }

    #[test]
    fn builtin_ltrim() {
        assert_eq!(
            eval_function_with_vals("ltrim", vec![Value::Str("  hello".into())]).unwrap(),
            Value::Str("hello".into())
        );
    }

    #[test]
    fn builtin_rtrim() {
        assert_eq!(
            eval_function_with_vals("rtrim", vec![Value::Str("hello  ".into())]).unwrap(),
            Value::Str("hello".into())
        );
    }

    #[test]
    fn builtin_left() {
        assert_eq!(
            eval_function_with_vals("left", vec![Value::Str("hello".into()), Value::Int(3)])
                .unwrap(),
            Value::Str("hel".into())
        );
    }

    #[test]
    fn builtin_right() {
        assert_eq!(
            eval_function_with_vals("right", vec![Value::Str("hello".into()), Value::Int(3)])
                .unwrap(),
            Value::Str("llo".into())
        );
    }

    #[test]
    fn builtin_substring_with_start_and_length() {
        assert_eq!(
            eval_function_with_vals(
                "substring",
                vec![Value::Str("hello".into()), Value::Int(1), Value::Int(3)]
            )
            .unwrap(),
            Value::Str("ell".into())
        );
    }

    #[test]
    fn builtin_replace() {
        assert_eq!(
            eval_function_with_vals(
                "replace",
                vec![
                    Value::Str("hello".into()),
                    Value::Str("l".into()),
                    Value::Str("r".into()),
                ]
            )
            .unwrap(),
            Value::Str("herro".into())
        );
    }

    #[test]
    fn builtin_split() {
        assert_eq!(
            eval_function_with_vals(
                "split",
                vec![Value::Str("a,b,c".into()), Value::Str(",".into())]
            )
            .unwrap(),
            Value::List(vec![
                Value::Str("a".into()),
                Value::Str("b".into()),
                Value::Str("c".into()),
            ])
        );
    }

    #[test]
    fn builtin_head() {
        assert_eq!(
            eval_function_with_vals(
                "head",
                vec![Value::List(vec![
                    Value::Int(1),
                    Value::Int(2),
                    Value::Int(3)
                ])]
            )
            .unwrap(),
            Value::Int(1)
        );
    }

    #[test]
    fn builtin_last() {
        assert_eq!(
            eval_function_with_vals(
                "last",
                vec![Value::List(vec![
                    Value::Int(1),
                    Value::Int(2),
                    Value::Int(3)
                ])]
            )
            .unwrap(),
            Value::Int(3)
        );
    }

    #[test]
    fn builtin_tail() {
        assert_eq!(
            eval_function_with_vals(
                "tail",
                vec![Value::List(vec![
                    Value::Int(1),
                    Value::Int(2),
                    Value::Int(3)
                ])]
            )
            .unwrap(),
            Value::List(vec![Value::Int(2), Value::Int(3)])
        );
    }

    #[test]
    fn builtin_reverse_list() {
        assert_eq!(
            eval_function_with_vals(
                "reverse",
                vec![Value::List(vec![
                    Value::Int(1),
                    Value::Int(2),
                    Value::Int(3)
                ])]
            )
            .unwrap(),
            Value::List(vec![Value::Int(3), Value::Int(2), Value::Int(1)])
        );
    }

    #[test]
    fn builtin_range_basic() {
        assert_eq!(
            eval_function_with_vals("range", vec![Value::Int(1), Value::Int(4)]).unwrap(),
            Value::List(vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3),
                Value::Int(4),
            ])
        );
    }

    #[test]
    fn builtin_keys_returns_non_null_keys() {
        use indexmap::IndexMap;
        let mut m: IndexMap<Box<str>, Value> = IndexMap::new();
        m.insert(Box::from("a"), Value::Int(1));
        m.insert(Box::from("b"), Value::Int(2));
        let result = eval_function_with_vals("keys", vec![Value::Map(m)]).unwrap();
        assert_eq!(
            result,
            Value::List(vec![Value::Str("a".into()), Value::Str("b".into())])
        );
    }

    #[test]
    fn builtin_properties_returns_map_clone() {
        use indexmap::IndexMap;
        let mut m: IndexMap<Box<str>, Value> = IndexMap::new();
        m.insert(Box::from("x"), Value::Int(42));
        let val = Value::Map(m.clone());
        let result = eval_function_with_vals("properties", vec![val]).unwrap();
        assert_eq!(result, Value::Map(m));
    }

    // ── Null three-valued logic ───────────────────────────────────────────────

    #[test]
    fn null_and_true_is_null() {
        let expr = Expr::BinaryOp {
            op: BOp::And,
            left: Box::new(Expr::Null),
            right: Box::new(Expr::Bool(true)),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Null);
    }

    #[test]
    fn null_and_false_is_false() {
        let expr = Expr::BinaryOp {
            op: BOp::And,
            left: Box::new(Expr::Null),
            right: Box::new(Expr::Bool(false)),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(false));
    }

    #[test]
    fn null_or_true_is_true() {
        let expr = Expr::BinaryOp {
            op: BOp::Or,
            left: Box::new(Expr::Null),
            right: Box::new(Expr::Bool(true)),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(true));
    }

    #[test]
    fn null_or_false_is_null() {
        let expr = Expr::BinaryOp {
            op: BOp::Or,
            left: Box::new(Expr::Null),
            right: Box::new(Expr::Bool(false)),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Null);
    }

    #[test]
    fn not_null_is_null() {
        let expr = Expr::UnaryOp {
            op: UOp::Not,
            expr: Box::new(Expr::Null),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Null);
    }

    #[test]
    fn in_list_with_null_element_not_found_is_null() {
        // 4 IN [1, null, 3] → null (element not present, but null exists)
        let expr = Expr::BinaryOp {
            op: BOp::In,
            left: Box::new(Expr::Integer(4)),
            right: Box::new(Expr::List(vec![
                Expr::Integer(1),
                Expr::Null,
                Expr::Integer(3),
            ])),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Null);
    }

    #[test]
    fn in_list_with_null_element_found_is_true() {
        // 3 IN [1, null, 3] → true (exact match found before null check matters)
        let expr = Expr::BinaryOp {
            op: BOp::In,
            left: Box::new(Expr::Integer(3)),
            right: Box::new(Expr::List(vec![
                Expr::Integer(1),
                Expr::Null,
                Expr::Integer(3),
            ])),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(true));
    }

    #[test]
    fn null_in_empty_list_is_false() {
        // null IN [] → false
        let expr = Expr::BinaryOp {
            op: BOp::In,
            left: Box::new(Expr::Null),
            right: Box::new(Expr::List(vec![])),
        };
        assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(false));
    }

    // ── Integer boundaries ────────────────────────────────────────────────────

    #[test]
    fn i64_max_plus_one_returns_overflow_error() {
        // Checked arithmetic: i64::MAX + 1 returns an overflow error.
        let expr = Expr::BinaryOp {
            op: BOp::Add,
            left: Box::new(Expr::Integer(i64::MAX)),
            right: Box::new(Expr::Integer(1)),
        };
        assert!(eval_expr(&expr, &empty_row()).is_err());
    }

    #[test]
    fn abs_i64_min_returns_overflow_error() {
        // Checked abs(): i64::MIN returns an overflow error instead of panicking.
        assert!(eval_function_with_vals("abs", vec![Value::Int(i64::MIN)]).is_err());
    }

    #[test]
    fn neg_i64_min_returns_overflow_error() {
        // Checked negation: i64::MIN returns an overflow error instead of panicking.
        let expr = Expr::UnaryOp {
            op: UOp::Neg,
            expr: Box::new(Expr::Integer(i64::MIN)),
        };
        assert!(eval_expr(&expr, &empty_row()).is_err());
    }

    #[test]
    #[should_panic]
    fn i64_min_div_neg_one_overflows() {
        // i64::MIN / -1 overflows (result doesn't fit in i64) in debug mode.
        let expr = Expr::BinaryOp {
            op: BOp::Div,
            left: Box::new(Expr::Integer(i64::MIN)),
            right: Box::new(Expr::Integer(-1)),
        };
        let _ = eval_expr(&expr, &empty_row());
    }

    // -- Regex cache eviction tests ------------------------------------------

    /// Helper: evaluate `string =~ pattern` and return the boolean result.
    fn eval_regex_match(string: &str, pattern: &str) -> Result<Value, GqlError> {
        let expr = Expr::BinaryOp {
            op: BOp::RegexMatch,
            left: Box::new(Expr::str_lit(string)),
            right: Box::new(Expr::str_lit(pattern)),
        };
        eval_expr(&expr, &empty_row())
    }

    #[test]
    fn regex_match_requires_full_string() {
        // Substring match must return false — Cypher =~ is implicitly anchored.
        assert_eq!(
            eval_regex_match("hello world", "hello").unwrap(),
            Value::Bool(false),
            "substring pattern should NOT match"
        );
    }

    #[test]
    fn regex_match_exact_string() {
        assert_eq!(
            eval_regex_match("hello", "hello").unwrap(),
            Value::Bool(true),
            "exact pattern should match the full string"
        );
    }

    #[test]
    fn regex_match_full_with_wildcard() {
        assert_eq!(
            eval_regex_match("hello", "hel.*").unwrap(),
            Value::Bool(true),
            "regex that covers the full string should match"
        );
    }

    #[test]
    fn regex_match_already_anchored() {
        assert_eq!(
            eval_regex_match("hello", "^hello$").unwrap(),
            Value::Bool(true),
            "already-anchored pattern should still work"
        );
    }

    #[test]
    fn regex_cache_basic_hit() {
        // First call compiles; second call should hit cache. Both return true.
        assert_eq!(eval_regex_match("abc", "^abc$").unwrap(), Value::Bool(true));
        assert_eq!(eval_regex_match("abc", "^abc$").unwrap(), Value::Bool(true));
    }

    #[test]
    fn regex_cache_eviction_preserves_recent_entries() {
        for i in 0..513 {
            let pattern = format!("^pattern_{i}$");
            let input = format!("pattern_{i}");
            assert_eq!(
                eval_regex_match(&input, &pattern).unwrap(),
                Value::Bool(true),
                "pattern_{i} should match"
            );
        }

        REGEX_CACHE.with(|cache| {
            let map = cache.borrow();
            assert!(
                map.len() <= 512,
                "cache should not exceed 512 entries, got {}",
                map.len()
            );
            assert!(
                map.len() > 1,
                "cache should retain entries after eviction, got {}",
                map.len()
            );
            // Cache keys are anchored: ^(?:<original>)$
            assert!(
                map.contains_key("^(?:^pattern_512$)$"),
                "most recent pattern should survive eviction"
            );
            assert!(
                !map.contains_key("^(?:^pattern_0$)$"),
                "oldest pattern should have been evicted"
            );
        });

        REGEX_CACHE.with(|cache| cache.borrow_mut().clear());
    }

    #[test]
    fn regex_cache_eviction_keeps_half() {
        for i in 0..512 {
            let pattern = format!("^evict_{i}$");
            let input = format!("evict_{i}");
            assert_eq!(
                eval_regex_match(&input, &pattern).unwrap(),
                Value::Bool(true)
            );
        }

        REGEX_CACHE.with(|cache| {
            assert_eq!(
                cache.borrow().len(),
                512,
                "cache should have 512 entries before eviction"
            );
        });

        assert_eq!(
            eval_regex_match("evict_512", "^evict_512$").unwrap(),
            Value::Bool(true)
        );

        REGEX_CACHE.with(|cache| {
            let map = cache.borrow();
            assert_eq!(
                map.len(),
                257,
                "after eviction cache should have 257 entries (256 survivors + 1 new)"
            );
            // Cache keys are anchored: ^(?:<original>)$
            assert!(!map.contains_key("^(?:^evict_0$)$"));
            assert!(!map.contains_key("^(?:^evict_255$)$"));
            assert!(map.contains_key("^(?:^evict_256$)$"));
            assert!(map.contains_key("^(?:^evict_511$)$"));
            assert!(map.contains_key("^(?:^evict_512$)$"));
        });

        REGEX_CACHE.with(|cache| cache.borrow_mut().clear());
    }
}
