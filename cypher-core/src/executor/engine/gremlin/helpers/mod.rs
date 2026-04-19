//! Gremlin executor: shared helpers and the `JavaCompatibleRandom` RNG.
//!
//! This module is split into focused submodules:
//! - [`random`]: Java-compatible RNG used by Gremlin sample/coin steps.
//! - [`value_format`]: Conversion of `Value`s to display/key strings and merging.
//! - [`predicate`]: Parsing and evaluation of Gremlin `P.*` predicate strings.
//! - [`operator`]: Structural inspection of the planner `Operator` tree.

mod operator;
mod predicate;
mod random;
mod value_format;

pub(super) use operator::{
    body_contains_simple_path, expand_bulk_rows, extract_simple_expand, operator_contains_barrier,
    operator_contains_distinct,
};
pub(super) use predicate::{
    eval_gremlin_predicate, eval_select_math_expr, parse_gremlin_option_args,
    parse_gremlin_option_value, trim_gremlin_option_string,
};
pub(super) use random::JavaCompatibleRandom;
pub(super) use value_format::{
    append_gremlin_debug_log, merge_gremlin_map_entry, summarize_gremlin_value_shape,
    value_to_map_key,
};
