//! Expression validation, type checking, aggregation helpers, and function validation.

mod aggregation;
mod arg_kinds;
mod functions;
mod general;
mod literals;
mod variables;

pub(super) use aggregation::{
    contains_aggregation, has_ambiguous_aggregation, has_nested_aggregation,
    is_aggregation_function,
};
pub(super) use literals::is_non_variable_expr;
pub(super) use variables::collect_variable_refs;
