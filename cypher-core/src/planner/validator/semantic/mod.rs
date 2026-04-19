//! Clause-level semantic validation for RETURN, WITH, UNION, and ORDER BY.
//!
//! Methods are spread across submodules and attach to `ValidationContext` via
//! `impl` blocks, following the same pattern as sibling modules
//! (`vars_check`, `pattern_check`, `skip_limit`, `helpers`).

mod order_by;
mod projection;
mod return_clause;
mod union;
mod with_clause;
