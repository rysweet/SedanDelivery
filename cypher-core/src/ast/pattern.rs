//! Pattern AST: nodes, relationships, paths, and inline properties.

use smol_str::SmolStr;

use super::common::{Direction, Identifier, Label, RelType};
use super::expr::Expr;

/// Shortest-path mode for a pattern wrapped in `shortestPath()` or `allShortestPaths()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShortestPathMode {
    /// `shortestPath((a)-[*]->(b))` — return one shortest path.
    Single,
    /// `allShortestPaths((a)-[*]->(b))` — return all paths of minimum length.
    All,
}

/// A path pattern such as `p = (a)-[r:KNOWS]->(b)`.
#[derive(Debug, Clone, PartialEq)]
pub struct PathPattern {
    /// Optional variable binding for the whole path (before `=`).
    pub variable: Option<Identifier>,
    /// The first node in the path.
    pub start: NodePattern,
    /// Zero or more alternating relationship + node steps.
    pub steps: Vec<PathStep>,
    /// When `Some`, this pattern was wrapped in `shortestPath()` or `allShortestPaths()`.
    pub shortest_path: Option<ShortestPathMode>,
}

/// A single step in a path: relationship followed by the next node.
#[derive(Debug, Clone, PartialEq)]
pub struct PathStep {
    pub rel: RelPattern,
    pub node: NodePattern,
}

/// A node pattern: `(variable:Label {props} WHERE predicate)`.
#[derive(Debug, Clone, PartialEq)]
pub struct NodePattern {
    pub variable: Option<Identifier>,
    pub labels: Vec<Label>,
    pub properties: Option<Properties>,
    pub where_clause: Option<Expr>,
}

/// A relationship pattern: `-[variable:TYPE*min..max {props} WHERE predicate]->`.
#[derive(Debug, Clone, PartialEq)]
pub struct RelPattern {
    pub variable: Option<Identifier>,
    /// Relationship types separated by `|`, e.g. `KNOWS|LIKES`.
    pub rel_types: Vec<RelType>,
    pub direction: Direction,
    pub range: Option<RangeQuantifier>,
    pub properties: Option<Properties>,
    pub where_clause: Option<Expr>,
}

/// Variable-length quantifier `*min..max`. Both bounds are optional.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeQuantifier {
    pub min: Option<u32>,
    pub max: Option<u32>,
}

/// An inline property map `{ key: expr, ... }` used in node/rel patterns.
pub type MapLiteral = Vec<(SmolStr, Expr)>;

/// Properties on a node/relationship pattern: either an inline map `{k: v, ...}`
/// or a parameter reference `$param_name` that resolves to a map at execution time.
#[derive(Debug, Clone, PartialEq)]
pub enum Properties {
    /// Inline property map: `{name: 'Alice', age: 30}`.
    Map(MapLiteral),
    /// Parameter reference: `$props` — must resolve to a map value at execution time.
    Parameter(SmolStr),
}
