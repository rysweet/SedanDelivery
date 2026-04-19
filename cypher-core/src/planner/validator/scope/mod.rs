//! Variable scope and binding validation for MATCH, CREATE, DELETE, UNWIND,
//! SET, REMOVE, and MERGE clauses.
//!
//! Each submodule contributes additional `impl` blocks to the
//! `ValidationContext` type. The public (super-visible) API is preserved.

mod create;
mod match_clause;
mod merge;
mod mutation;
mod where_clause;
