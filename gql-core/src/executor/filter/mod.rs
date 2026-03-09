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
