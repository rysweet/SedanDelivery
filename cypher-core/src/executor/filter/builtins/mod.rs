//! Builtin function implementations: math, string, type conversion, collection functions.
//!
//! Organized by category into focused submodules:
//!
//! - `helpers`: shared arity/type validation utilities
//! - `numeric`: math and random functions
//! - `coercion`: `toInteger`, `toFloat`, `toBoolean`, `toString`
//! - `string`: case, trimming, slicing, search/replace
//! - `list`: head/last/tail/reverse/range
//! - `map`: keys/values/properties
//! - `null_handling`: coalesce / isNull

mod coercion;
mod helpers;
mod list;
mod map;
mod null_handling;
mod numeric;
mod string;

pub(super) use coercion::{
    builtin_toboolean, builtin_tofloat, builtin_tointeger, builtin_tostring,
};
pub(super) use helpers::type_error;
pub(super) use list::{builtin_head, builtin_last, builtin_range, builtin_reverse, builtin_tail};
pub(super) use map::{builtin_keys, builtin_properties, builtin_values};
pub(super) use null_handling::{builtin_coalesce, builtin_isnull};
pub(super) use numeric::{
    builtin_abs, builtin_acos, builtin_asin, builtin_atan, builtin_atan2, builtin_ceil,
    builtin_cos, builtin_degrees, builtin_e, builtin_exp, builtin_floor,
    builtin_gremlin_sample_local, builtin_gremlin_sample_weighted_key, builtin_haversin,
    builtin_java_random_next_double, builtin_log, builtin_log10, builtin_pi, builtin_radians,
    builtin_rand, builtin_round, builtin_sign, builtin_sin, builtin_sqrt, builtin_tan,
};
pub(super) use string::{
    builtin_left, builtin_length, builtin_ltrim, builtin_replace, builtin_right, builtin_rtrim,
    builtin_size, builtin_split, builtin_substring, builtin_tolower, builtin_toupper, builtin_trim,
};

#[cfg(test)]
mod tests;
