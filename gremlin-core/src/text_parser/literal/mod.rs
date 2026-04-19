//! Literal value, collection, and argument-list parsing.
//!
//! Decomposed into focused submodules; the public surface is preserved as
//! flat re-exports so callers can continue using `super::literal::*`.

mod collection;
mod string;
mod value;

pub(crate) use collection::{
    parse_arg_list, parse_collection_step_arg, parse_string_arg_list,
    parse_string_arg_list_with_params,
};
pub(crate) use string::{normalize_gtype, resolve_constant_traversal, unquote};
pub(crate) use value::{parse_literal_value, parse_single_value};
