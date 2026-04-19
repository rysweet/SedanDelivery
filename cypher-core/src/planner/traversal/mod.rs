//! Operator-tree traversal helpers.
//!
//! Provides `Operator::children` for structural traversal and
//! `collect_visible_aliases` for scope analysis.

mod aliases;
mod children;

pub use aliases::collect_visible_aliases;
