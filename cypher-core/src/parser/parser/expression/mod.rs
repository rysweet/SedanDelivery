//! Expression parsing with operator precedence, unary/binary ops, function calls,
//! subscripts, slicing, list comprehensions, CASE expressions.
//!
//! Decomposed into topic-named submodules. Each submodule contributes
//! additional inherent methods to `super::Parser` via its own `impl` block.

mod calls;
mod case_exists;
mod operators;
mod patterns;
mod postfix;
mod primary;
