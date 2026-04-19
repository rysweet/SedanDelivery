//! Top-level clause AST: queries, clauses, and clause bodies.

use smol_str::SmolStr;

use super::common::{Identifier, Label};
use super::expr::Expr;
use super::pattern::PathPattern;

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
