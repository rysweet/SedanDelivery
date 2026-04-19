//! Gremlin function implementations extracted from `filter::eval_function_with_vals`.
//!
//! These were ~1670 lines of inline match-arm bodies in the original mod.rs.
//! They are split here by category for readability; behavior is unchanged.

pub(crate) mod collection;
pub(crate) mod column;
pub(crate) mod concat;
pub(crate) mod predicates;
pub(crate) mod reduce;
pub(crate) mod string;
pub(crate) mod typeconv;
