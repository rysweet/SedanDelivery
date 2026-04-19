//! Helper functions for the query planner — expression utilities, property extraction,
//! aggregation detection, and column name derivation.
//!
//! This module is split into topic-focused submodules; the public (crate-internal) API
//! is preserved by re-exporting all helpers from this `mod.rs`.

mod aggregation;
mod column_name;
mod defaults;
mod literal;
mod order_rewrite;
mod properties;
mod set_remove;
mod star;

pub(crate) use aggregation::{
    contains_aggregation, extract_agg_function, find_inner_aggregate, has_agg_function,
    rewrite_agg_to_var,
};
pub(crate) use column_name::{expr_to_col_name, return_item_name};
pub(crate) use defaults::{empty_params, empty_registry, PathBinding};
pub(crate) use literal::expr_to_literal_value;
pub(crate) use order_rewrite::{rewrite_agg_order_expr, rewrite_with_order_expr};
pub(crate) use properties::{
    label_filter_expr, param_props_filter_expr, properties_to_prop_pairs, props_to_filter_expr,
};
pub(crate) use set_remove::{map_remove_clause_item, map_set_clause_item};
pub(crate) use star::expand_star_items;
