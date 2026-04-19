//! Gremlin-specific helper types referenced by `Operator::GremlinSelect`.

use crate::ast::Identifier;

use super::operator::Operator;

/// Pop modifier for Gremlin select() — controls which labeled value to
/// return when a label has been assigned multiple times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GremlinPop {
    First,
    Last,
    All,
    Mixed,
}

#[derive(Debug, Clone)]
pub enum GremlinSelectTraversal {
    /// Evaluate a compiled by(traversal) sub-traversal for the selected key.
    Traversal {
        op: Box<Operator>,
        out_alias: Identifier,
    },
}
