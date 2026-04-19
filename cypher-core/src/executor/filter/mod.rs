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
mod case;
mod collections;
mod expr;
mod geo;
mod gremlin;
mod gremlin_pred;
mod map_key;
mod math;
pub(crate) mod operators;
mod query_time;
mod temporal;

#[cfg(test)]
mod tests;

pub(crate) use self::case::eval_case;
pub(crate) use self::collections::quantifier_result;
pub use self::expr::eval_expr;
pub use self::expr::Row;
pub(crate) use self::map_key::map_key_to_value;
pub(crate) use self::operators::cypher_compare;
pub use self::query_time::{clear_query_now, init_query_now, QueryTimeGuard};
pub(crate) use self::query_time::{get_query_now, REGEX_CACHE};
pub(crate) use self::temporal::time_to_utc_nanos;
pub use self::temporal::{
    duration_add, duration_div, duration_mul, duration_mul_f64, duration_sub,
    temporal_add_duration, temporal_property_access,
};

use crate::ast::Expr;
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

/// Hard cap on list sizes produced by range() and split() to prevent memory exhaustion DoS.
/// Reads from the deployment-time config (env `GQL_MAX_LIST_SIZE`, default 10 million).
#[inline]
pub(crate) fn max_list_size() -> usize {
    crate::config::global().max_list_size
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
        "java_random_next_double" => builtins::builtin_java_random_next_double(vals),
        "gremlin_sample_weighted_key" => builtins::builtin_gremlin_sample_weighted_key(vals),
        "gremlin_sample_local" => builtins::builtin_gremlin_sample_local(vals),
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
        "gremlin_tolower" => gremlin::string::tolower(vals),
        "gremlin_toupper" => gremlin::string::toupper(vals),
        "gremlin_trim" => gremlin::string::trim(vals),
        "gremlin_ltrim" => gremlin::string::ltrim(vals),
        "gremlin_rtrim" => gremlin::string::rtrim(vals),
        "gremlin_length" => gremlin::string::length(vals),
        "gremlin_reverse" => gremlin::string::reverse(vals),
        "gremlin_replace" => gremlin::string::replace(vals),
        "gremlin_split" => gremlin::string::split(vals),
        "gremlin_substring" => gremlin::string::substring(vals),
        "gremlin_substring_local" => gremlin::string::substring_local(vals),
        "gremlin_tolower_local" => gremlin::string::tolower_local(vals),
        "gremlin_toupper_local" => gremlin::string::toupper_local(vals),
        "gremlin_trim_local" => gremlin::string::trim_local(vals),
        "gremlin_ltrim_local" => gremlin::string::ltrim_local(vals),
        "gremlin_rtrim_local" => gremlin::string::rtrim_local(vals),
        "gremlin_replace_local" => gremlin::string::replace_local(vals),
        "gremlin_split_local" => gremlin::string::split_local(vals),
        "gremlin_count_local" => gremlin::string::count_local(vals),
        "gremlin_length_local" => gremlin::string::length_local(vals),
        "gremlin_fold_reduce" => gremlin::reduce::fold_reduce(vals),
        "gremlin_math" => gremlin::reduce::math(vals),
        "gremlin_tail_local" => gremlin::reduce::tail_local(vals),
        "gremlin_dedup_local" => gremlin::reduce::dedup_local(vals),
        "gremlin_concat" => gremlin::concat::concat(vals),
        "gremlin_conjoin" => gremlin::concat::conjoin(vals),
        "gremlin_format" => gremlin::concat::format(vals),
        "gremlin_format_by" => gremlin::concat::format_by(vals),
        "gremlin_make_datetime" => gremlin::concat::make_datetime(vals),
        "gremlin_make_uuid" => gremlin::concat::make_uuid(vals),
        "gremlin_asstring" => gremlin::typeconv::asstring(vals),
        "gremlin_asdate" => gremlin::typeconv::asdate(vals),
        "gremlin_asstring_local" => gremlin::typeconv::asstring_local(vals),
        "gremlin_asnumber" => gremlin::typeconv::asnumber(vals),
        "gremlin_asnumber_typed" => gremlin::typeconv::asnumber_typed(vals),
        "gremlin_asbool" => gremlin::typeconv::asbool(vals),
        "gremlin_typeof" => gremlin::typeconv::type_of(vals),
        "gremlin_combine" => gremlin::collection::combine(vals),
        "gremlin_merge" => gremlin::collection::merge(vals),
        "gremlin_intersect" => gremlin::collection::intersect(vals),
        "gremlin_difference" => gremlin::collection::difference(vals),
        "gremlin_disjunct" => gremlin::collection::disjunct(vals),
        "gremlin_product" => gremlin::collection::product(vals),
        "gremlin_all" => gremlin::collection::all(vals),
        "gremlin_any" => gremlin::collection::any(vals),
        "gremlin_none" => gremlin::collection::none(vals),
        "gremlin_wraplist" => gremlin::collection::wraplist(vals),
        "gremlin_valuemap" => gremlin::collection::valuemap(vals),
        "gremlin_valuemap_unfold" => gremlin::collection::valuemap_unfold(vals),
        "list_compact" => {
            let result: Vec<Value> = vals
                .into_iter()
                .filter(|v| !matches!(v, Value::Null))
                .collect();
            Ok(Value::List(result))
        }
        "gremlin_values_all" => gremlin::collection::values_all(vals),
        "gremlin_column_key" | "gremlin_unfold_map_entry_key" => gremlin::column::column_key(vals),
        "gremlin_select_values" => gremlin::column::select_values(vals),
        "gremlin_column_value" | "gremlin_unfold_map_entry_value" => {
            gremlin::column::column_value(vals)
        }
        "gremlin_column_values" => gremlin::column::column_values(vals),
        "gremlin_column_keys" => gremlin::column::column_keys(vals),
        "gremlin_sort_local" => gremlin::column::sort_local(vals),
        "gremlin_sum_local" => gremlin::column::sum_local(vals),
        "gremlin_min_local" => gremlin::column::min_local(vals),
        "gremlin_max_local" => gremlin::column::max_local(vals),
        "gremlin_mean_local" => gremlin::column::mean_local(vals),
        "gremlin_containing" => gremlin::predicates::containing(vals),
        "gremlin_startingwith" => gremlin::predicates::startingwith(vals),
        "gremlin_endingwith" => gremlin::predicates::endingwith(vals),
        "gremlin_regex" => gremlin::predicates::regex(vals),
        "gremlin_istype" => gremlin::predicates::istype(vals),
        // ── Unknown function ──────────────────────────────────────────────────
        other => Err(GqlError::Execution(ExecutionError::unsupported(format!(
            "unknown function '{other}'"
        )))),
    }
}
