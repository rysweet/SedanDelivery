//! List comprehension, subscript, slice, quantifier predicates (ALL, ANY, NONE, SINGLE).
//!
//! This module groups collection-related expression evaluators used by the
//! filter pipeline. It is split into focused submodules:
//!
//! - [`subscript`] — `list[i]` / `map[key]` indexing
//! - [`slice`] — `list[start..end]` slicing
//! - [`comprehension`] — list comprehension and `reduce(...)`
//! - [`quantifier`] — `all`/`any`/`none`/`single` predicates

mod comprehension;
mod quantifier;
mod slice;
mod subscript;

#[cfg(test)]
mod tests;

pub(super) use comprehension::{eval_list_comp, eval_reduce};
pub(super) use quantifier::eval_quantifier;
pub(crate) use quantifier::quantifier_result;
pub(super) use slice::eval_slice;
pub(super) use subscript::eval_subscript;
