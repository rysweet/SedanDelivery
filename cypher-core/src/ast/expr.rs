//! Cypher expression AST: operators and the [`Expr`] enum.

use smol_str::SmolStr;

use super::common::{Identifier, Label};
use super::pattern::PathPattern;

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
