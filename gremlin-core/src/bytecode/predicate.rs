//! The [`Predicate`] enum representing Gremlin comparison predicates.

use crate::types::GremlinValue;
use serde::{Deserialize, Serialize};

/// A Gremlin predicate for comparison operations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Predicate {
    Eq(GremlinValue),
    Neq(GremlinValue),
    Gt(GremlinValue),
    Gte(GremlinValue),
    Lt(GremlinValue),
    Lte(GremlinValue),
    Within(Vec<GremlinValue>),
    Without(Vec<GremlinValue>),
    Between(GremlinValue, GremlinValue),
    /// `P.inside(low, high)` — exclusive range (low < x < high)
    Inside(GremlinValue, GremlinValue),
    // Text predicates
    Containing(GremlinValue),
    NotContaining(GremlinValue),
    StartingWith(GremlinValue),
    NotStartingWith(GremlinValue),
    EndingWith(GremlinValue),
    NotEndingWith(GremlinValue),
    Regex(GremlinValue),
    NotRegex(GremlinValue),
    /// `P.typeOf(GType.STRING)` — check if value is of the specified Gremlin type
    IsType(GremlinValue),
    /// Compound predicate: all must match (P.gt(1).and(P.lt(5)))
    And(Vec<Predicate>),
    /// Compound predicate: any must match (P.gt(5).or(P.lt(1)))
    Or(Vec<Predicate>),
    /// Negation of a predicate: P.not(P.eq(1))
    Not(Box<Predicate>),
}
