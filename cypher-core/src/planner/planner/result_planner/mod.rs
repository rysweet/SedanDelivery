//! Result planning — RETURN, WITH, UNWIND, CALL, aggregation extraction, ORDER BY,
//! DISTINCT/SKIP/LIMIT.
//!
//! This module is decomposed by clause/responsibility:
//! - [`call`]: CALL clause planning
//! - [`with_clause`]: WITH clause planning (incl. aggregation handling)
//! - [`unwind`]: UNWIND clause planning
//! - [`return_clause`]: RETURN clause planning
//! - [`agg`]: shared aggregation extraction used by RETURN/WITH

mod agg;
mod call;
mod return_clause;
mod unwind;
mod with_clause;
