//! Abstract Syntax Tree types for the GQL/Cypher engine.
//!
//! All heap-allocated string data uses [`smol_str::SmolStr`] for labels, relationship types,
//! and property keys (inline storage for strings up to 22 bytes, heap otherwise). Full
//! identifiers use `String` since they can be arbitrarily long.

use std::ops::Deref;

use smol_str::SmolStr;

// ── Source spans ──────────────────────────────────────────────────────────────

/// A half-open byte range `[start, end)` into the original query string.
///
/// Used for error reporting: when the query planner or executor detects an
/// error, it attaches the `Span` of the offending AST node so the error
/// message can point at the right part of the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// A span covering zero bytes, used as a sentinel when no source position is available.
    pub fn zero() -> Self {
        Self { start: 0, end: 0 }
    }

    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

// ── Spanned<T> ────────────────────────────────────────────────────────────────

/// Wraps an AST node `T` with its source [`Span`].
///
/// Implements [`Deref<Target = T>`] so code that doesn't care about spans can
/// reach through transparently.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub fn new(node: T, span: Span) -> Self {
        Self { node, span }
    }

    pub fn without_span(node: T) -> Self {
        Self {
            node,
            span: Span::zero(),
        }
    }
}

impl<T> Deref for Spanned<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.node
    }
}

// ── Identifier / Label / RelType newtypes ────────────────────────────────────

/// A Cypher variable name or property key, such as `n`, `r`, `person`.
///
/// Stored as a `String` because variable names can be backtick-quoted and
/// arbitrarily long.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Identifier(pub String);

impl Identifier {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Identifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for Identifier {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for Identifier {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// A node label, such as `Person` or `Movie`.
///
/// Uses [`SmolStr`] for inline storage: labels are almost always short ASCII
/// strings that fit in 22 bytes, avoiding a heap allocation per label.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Label(pub SmolStr);

impl Label {
    pub fn new(s: impl Into<SmolStr>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for Label {
    fn from(s: &str) -> Self {
        Self(SmolStr::new(s))
    }
}

impl From<String> for Label {
    fn from(s: String) -> Self {
        Self(SmolStr::new(s))
    }
}

/// A relationship type, such as `KNOWS` or `ACTED_IN`.
///
/// Uses [`SmolStr`] for the same reason as [`Label`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RelType(pub SmolStr);

impl RelType {
    pub fn new(s: impl Into<SmolStr>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for RelType {
    fn from(s: &str) -> Self {
        Self(SmolStr::new(s))
    }
}

impl From<String> for RelType {
    fn from(s: String) -> Self {
        Self(SmolStr::new(s))
    }
}

// ── Direction ─────────────────────────────────────────────────────────────────

/// Direction of a relationship pattern.
///
/// ```text
/// (a)-[r]->(b)   → Outgoing  (from a's perspective)
/// (a)<-[r]-(b)   → Incoming  (from a's perspective)
/// (a)-[r]-(b)    → Undirected
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// `(a)-[r]->(b)` — follows edges from src to dst.
    Outgoing,
    /// `(a)<-[r]-(b)` — follows edges from dst to src.
    Incoming,
    /// `(a)-[r]-(b)` — follows edges in either direction.
    Undirected,
}

// ── Binary operators ──────────────────────────────────────────────────────────

/// Binary operators in Cypher expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,

    // Comparison
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,

    // Logical
    And,
    Or,
    Xor,

    // String
    Concat, // `+` on strings (reuses Add at the AST level; kept separate for clarity)
    StartsWith,
    EndsWith,
    Contains,
    RegexMatch, // `=~`

    // List
    In, // `x IN list`
}

// ── Unary operators ───────────────────────────────────────────────────────────

/// Unary operators in Cypher expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    /// `-x`
    Neg,
    /// `NOT x`
    Not,
}

// ── Expressions ──────────────────────────────────────────────────────────────

/// A Cypher expression.
///
/// Expressions can appear in WHERE clauses, RETURN projections, SET assignments,
/// CREATE property maps, etc.
///
/// Design notes:
/// - Strings are `Box<str>` (16 bytes vs `String`'s 24 bytes; immutable once parsed).
/// - Property keys in `Property` and `MapLiteral` are `SmolStr` for inline storage.
/// - `Box<Expr>` avoids infinite size; only used where recursion requires it.
/// - `FunctionCall.name` is `SmolStr` because function names are short.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    // ── Literals ──────────────────────────────────────────────────────────────
    Integer(i64),
    Float(f64),
    Str(Box<str>),
    Bool(bool),
    Null,

    // ── Variable reference ────────────────────────────────────────────────────
    /// Reference to a variable in scope, e.g. `n`, `r`.
    Variable(Identifier),

    // ── Property access ───────────────────────────────────────────────────────
    /// `expr.key` — accesses a property on a node, edge, or map.
    Property(Box<Expr>, SmolStr),

    // ── Operators ─────────────────────────────────────────────────────────────
    BinaryOp {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },

    UnaryOp {
        op: UnaryOp,
        expr: Box<Expr>,
    },

    // ── Null checks ───────────────────────────────────────────────────────────
    IsNull(Box<Expr>),
    IsNotNull(Box<Expr>),

    // ── Label check ──────────────────────────────────────────────────────────
    /// `a:Label` — checks if a node has a specific label.
    HasLabel(Box<Expr>, Label),

    // ── Function calls ────────────────────────────────────────────────────────
    FunctionCall {
        name: SmolStr,
        args: Vec<Expr>,
        /// `true` for `count(DISTINCT x)`, `collect(DISTINCT x)`, etc.
        distinct: bool,
    },

    // ── Parameters ───────────────────────────────────────────────────────────
    /// A query parameter, e.g. `$userId` or `$0`.
    Parameter(String),

    // ── Collections ──────────────────────────────────────────────────────────
    /// A list literal: `[1, 2, 3]` or `[x IN range(1,10) WHERE x > 5 | x * 2]`.
    List(Vec<Expr>),

    /// A map literal: `{name: 'Alice', age: 30}`.
    MapLiteral(Vec<(SmolStr, Expr)>),

    // ── CASE expression ──────────────────────────────────────────────────────
    /// `CASE [subject] WHEN cond THEN result ... [ELSE default] END`
    Case {
        /// `None` for a generic CASE (each WHEN is a full predicate);
        /// `Some(expr)` for a value-comparison CASE.
        subject: Option<Box<Expr>>,
        alternatives: Vec<(Expr, Expr)>, // (WHEN condition, THEN result)
        default: Option<Box<Expr>>,
    },

    // ── Subscript / slice ─────────────────────────────────────────────────────
    /// `list[index]` — zero-based list element access.
    Subscript(Box<Expr>, Box<Expr>),

    /// `list[start..end]` — list slice (either bound may be None).
    Slice(Box<Expr>, Option<Box<Expr>>, Option<Box<Expr>>),

    // ── List comprehension ────────────────────────────────────────────────────
    /// `[variable IN list WHERE filter | map]`
    ///
    /// - `variable`: the loop variable bound to each element.
    /// - `list`: the source list expression.
    /// - `filter`: an optional predicate; when `None` all elements pass.
    /// - `map`: the projection applied to each surviving element.
    ///
    /// Evaluating this expression:
    /// 1. Evaluates `list` to a `Value::List`.
    /// 2. For each element, binds `variable` and evaluates `filter` (if present).
    ///    Elements for which the filter is false or null are dropped.
    /// 3. Maps each surviving element through `map` with `variable` bound.
    /// 4. Returns a new `Value::List` of the mapped values.
    ListComp {
        variable: Identifier,
        list: Box<Expr>,
        filter: Option<Box<Expr>>,
        map: Box<Expr>,
    },

    // ── Reduce expression ─────────────────────────────────────────────────────
    /// `reduce(accumulator = initial, variable IN list | expression)`
    ///
    /// Folds over a list:
    /// 1. Binds `accumulator` to `initial`.
    /// 2. For each element in `list`, binds `variable` to the element and
    ///    evaluates `expression` (which may reference both `accumulator` and
    ///    `variable`). The result replaces the accumulator value.
    /// 3. Returns the final accumulator value.
    Reduce {
        accumulator: Identifier,
        initial: Box<Expr>,
        variable: Identifier,
        list: Box<Expr>,
        expression: Box<Expr>,
    },

    // ── Existential subquery ─────────────────────────────────────────────────
    /// `exists { pattern [WHERE predicate] }` — existential subquery.
    /// When `full_query` is Some, the EXISTS contains a full inner query (with WITH/RETURN).
    ExistsSubquery {
        patterns: Vec<PathPattern>,
        where_clause: Option<Box<Expr>>,
        /// Full inner query (WITH/RETURN clauses) when present; None for simple pattern form.
        full_query: Option<Box<crate::ast::Query>>,
    },
    // ── Pattern expression ──────────────────────────────────────────────────
    /// `(n)-[:REL]->()` used as a predicate in WHERE — returns true if the
    /// pattern matches at least once.
    PatternExpr(Vec<PathPattern>),
}

impl Expr {
    /// Convenience constructor for string literals.
    pub fn str_lit(s: impl Into<Box<str>>) -> Self {
        Self::Str(s.into())
    }

    /// Returns `true` if this expression is a literal that cannot refer to any variable.
    pub fn is_literal(&self) -> bool {
        matches!(
            self,
            Self::Integer(_) | Self::Float(_) | Self::Str(_) | Self::Bool(_) | Self::Null
        )
    }
}

// ── Clause-level AST types ────────────────────────────────────────────────────

/// A complete parsed query, consisting of one or more clauses,
/// optionally composed via UNION / UNION ALL.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub clauses: Vec<Clause>,
    /// If non-empty, subsequent UNION parts (each a list of clauses).
    /// `unions[i].0` is true for UNION ALL, false for plain UNION.
    #[doc(hidden)]
    pub unions: Vec<(bool, Vec<Clause>)>,
}

impl Query {
    /// Create a simple query with no UNION parts.
    pub fn new(clauses: Vec<Clause>) -> Self {
        Self {
            clauses,
            unions: Vec::new(),
        }
    }
}

/// A single top-level Cypher/GQL clause.
#[derive(Debug, Clone, PartialEq)]
pub enum Clause {
    Match(MatchClause),
    OptionalMatch(MatchClause),
    Where(Expr),
    Return(ReturnClause),
    Create(CreateClause),
    Delete(DeleteClause),
    With(WithClause),
    Unwind(UnwindClause),
    Set(SetClause),
    Remove(RemoveClause),
    Merge(MergeClause),
    Call(CallClause),
    /// `CALL { ... }` — inline subquery block.
    CallSubquery {
        inner: Vec<Clause>,
    },
    Foreach(ForeachClause),
    LoadCsv(LoadCsvClause),
}

/// A LOAD CSV clause: `LOAD CSV [WITH HEADERS] FROM 'file:///path.csv' AS row`.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadCsvClause {
    /// Whether `WITH HEADERS` was specified.
    pub with_headers: bool,
    /// The source expression (must evaluate to a string URI).
    pub source: Expr,
    /// The alias for each row.
    pub alias: Identifier,
}

/// A MATCH or OPTIONAL MATCH clause.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchClause {
    pub patterns: Vec<PathPattern>,
    pub where_clause: Option<Expr>,
}

/// A RETURN clause with optional modifiers.
#[derive(Debug, Clone, PartialEq)]
pub struct ReturnClause {
    pub distinct: bool,
    pub items: Vec<ReturnItem>,
    pub order_by: Vec<SortItem>,
    pub skip: Option<Expr>,
    pub limit: Option<Expr>,
}

/// A single expression in a RETURN or WITH projection.
#[derive(Debug, Clone, PartialEq)]
pub struct ReturnItem {
    pub expr: Expr,
    pub alias: Option<Identifier>,
}

/// A single item in an ORDER BY clause.
#[derive(Debug, Clone, PartialEq)]
pub struct SortItem {
    pub expr: Expr,
    pub ascending: bool,
}

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

/// A CREATE clause.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateClause {
    pub patterns: Vec<PathPattern>,
}

/// A DELETE or DETACH DELETE clause.
#[derive(Debug, Clone, PartialEq)]
pub struct DeleteClause {
    pub exprs: Vec<Expr>,
    pub detach: bool,
}

/// A SET clause.
#[derive(Debug, Clone, PartialEq)]
pub struct SetClause {
    pub items: Vec<SetClauseItem>,
}

/// A REMOVE clause.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoveClause {
    pub items: Vec<RemoveClauseItem>,
}

/// A MERGE clause with optional ON CREATE SET / ON MATCH SET actions.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeClause {
    pub pattern: PathPattern,
    pub on_create: Vec<SetClauseItem>,
    pub on_match: Vec<SetClauseItem>,
}

/// A WITH clause (like RETURN but feeds the next query part).
#[derive(Debug, Clone, PartialEq)]
pub struct WithClause {
    pub distinct: bool,
    pub items: Vec<ReturnItem>,
    pub where_clause: Option<Expr>,
    pub order_by: Vec<SortItem>,
    pub skip: Option<Expr>,
    pub limit: Option<Expr>,
}

/// An UNWIND clause.
#[derive(Debug, Clone, PartialEq)]
pub struct UnwindClause {
    pub expr: Expr,
    pub alias: Identifier,
}

/// A CALL clause: `CALL procedure.name(args) YIELD var1 AS alias1, var2`.
#[derive(Debug, Clone, PartialEq)]
pub struct CallClause {
    pub procedure: String,
    pub args: Vec<Expr>,
    /// YIELD items: `(name, optional_alias)`.
    pub yield_items: Vec<(Identifier, Option<Identifier>)>,
    /// Whether arguments were passed explicitly with parentheses `()`.
    pub explicit_args: bool,
    /// Whether `YIELD *` was used.
    pub yield_star: bool,
}

/// A FOREACH clause: `FOREACH (x IN list | CREATE ...)`.
#[derive(Debug, Clone, PartialEq)]
pub struct ForeachClause {
    pub variable: Identifier,
    pub list: Expr,
    pub clauses: Vec<Clause>,
}

/// A single item in a SET clause.
#[derive(Debug, Clone, PartialEq)]
pub enum SetClauseItem {
    /// `n.prop = expr`
    SetProperty {
        alias: Identifier,
        prop: SmolStr,
        value: Expr,
    },
    /// `n = expr` (replace map) or `n += expr` (merge map).
    SetMap {
        alias: Identifier,
        map: Expr,
        merge: bool,
    },
    /// `n:Label1:Label2`
    SetLabels {
        alias: Identifier,
        labels: Vec<Label>,
    },
}

/// A single item in a REMOVE clause.
#[derive(Debug, Clone, PartialEq)]
pub enum RemoveClauseItem {
    /// `n.prop`
    RemoveProperty { alias: Identifier, prop: SmolStr },
    /// `n:Label1:Label2`
    RemoveLabels {
        alias: Identifier,
        labels: Vec<Label>,
    },
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_len_and_is_empty() {
        let s = Span::new(3, 7);
        assert_eq!(s.len(), 4);
        assert!(!s.is_empty());

        let z = Span::zero();
        assert_eq!(z.len(), 0);
        assert!(z.is_empty());
    }

    #[test]
    fn spanned_deref() {
        let s: Spanned<i32> = Spanned::new(42, Span::zero());
        assert_eq!(*s, 42);
    }

    #[test]
    fn identifier_eq_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(Identifier::new("foo"));
        assert!(set.contains(&Identifier::new("foo")));
        assert!(!set.contains(&Identifier::new("bar")));
    }

    #[test]
    fn label_eq_hash() {
        use std::collections::HashSet;
        let l1 = Label::new("Person");
        let l2 = Label::from("Person");
        let mut set = HashSet::new();
        set.insert(l1.clone());
        assert!(set.contains(&l2));
    }

    #[test]
    fn rel_type_from_string() {
        let r = RelType::from(String::from("KNOWS"));
        assert_eq!(r.as_str(), "KNOWS");
    }

    #[test]
    fn direction_variants_are_distinct() {
        assert_ne!(Direction::Outgoing, Direction::Incoming);
        assert_ne!(Direction::Incoming, Direction::Undirected);
    }

    #[test]
    fn expr_is_literal() {
        assert!(Expr::Integer(1).is_literal());
        assert!(Expr::Float(1.23456).is_literal());
        assert!(Expr::str_lit("hello").is_literal());
        assert!(Expr::Bool(false).is_literal());
        assert!(Expr::Null.is_literal());
        assert!(!Expr::Variable(Identifier::new("n")).is_literal());
    }

    #[test]
    fn binary_op_eq() {
        assert_eq!(BinaryOp::Add, BinaryOp::Add);
        assert_ne!(BinaryOp::Add, BinaryOp::Sub);
    }

    #[test]
    fn expr_property_access() {
        let expr = Expr::Property(
            Box::new(Expr::Variable(Identifier::new("n"))),
            SmolStr::new("name"),
        );
        match expr {
            Expr::Property(inner, key) => {
                assert_eq!(key, "name");
                assert!(matches!(*inner, Expr::Variable(_)));
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn expr_function_call() {
        let expr = Expr::FunctionCall {
            name: SmolStr::new("count"),
            args: vec![Expr::Variable(Identifier::new("n"))],
            distinct: true,
        };
        match expr {
            Expr::FunctionCall {
                name,
                args,
                distinct,
            } => {
                assert_eq!(name, "count");
                assert!(distinct);
                assert_eq!(args.len(), 1);
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn expr_map_literal() {
        let expr = Expr::MapLiteral(vec![
            (SmolStr::new("name"), Expr::str_lit("Alice")),
            (SmolStr::new("age"), Expr::Integer(30)),
        ]);
        match expr {
            Expr::MapLiteral(pairs) => {
                assert_eq!(pairs.len(), 2);
                assert_eq!(pairs[0].0, "name");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn expr_binary_op_structure() {
        let expr = Expr::BinaryOp {
            op: BinaryOp::Add,
            left: Box::new(Expr::Integer(1)),
            right: Box::new(Expr::Integer(2)),
        };
        match &expr {
            Expr::BinaryOp { op, left, right } => {
                assert_eq!(*op, BinaryOp::Add);
                assert!(matches!(**left, Expr::Integer(1)));
                assert!(matches!(**right, Expr::Integer(2)));
            }
            _ => panic!("unexpected variant"),
        }
    }
}
