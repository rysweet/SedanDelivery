//! Gremlin collection functions extracted from filter::eval_function_with_vals.
//!
//! Behavior preserved verbatim from the original `filter` module.
//!
//! Submodules:
//! - [`set_ops`]: list/multiset/map combine, merge, intersect, difference,
//!   disjunct, product
//! - [`predicates`]: collection predicates `all`, `any`, `none`
//! - [`valuemap`]: TinkerPop `valueMap` / `wraplist` / `values_all` helpers

#![allow(clippy::needless_pass_by_value)]
#![allow(unused_imports)]

mod predicates;
mod set_ops;
mod valuemap;

pub(crate) use predicates::{all, any, none};
pub(crate) use set_ops::{combine, difference, disjunct, intersect, merge, product};
pub(crate) use valuemap::{valuemap, valuemap_unfold, values_all, wraplist};
