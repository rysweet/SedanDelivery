//! Free helper functions used by the Gremlin compiler.
//!
//! Originally a single 1812-line `helpers.rs`; split into focused submodules
//! grouped by concern. The submodules are private to `compiler` and re-export
//! their items so existing `use helpers::*;` imports keep working unchanged.

mod conversion;
mod extract;
mod operators;
mod predicate_expr;
mod predicate_label;
mod predicates;
mod steps;
mod where_by;

pub(super) use conversion::*;
pub(super) use extract::*;
pub(super) use operators::*;
pub(super) use predicate_expr::*;
pub(super) use predicate_label::*;
pub(super) use predicates::*;
pub(super) use steps::*;
pub(super) use where_by::*;
