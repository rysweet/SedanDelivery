//! Compile-time query validator for openCypher / GQL.
//!
//! [`validate_query`] runs after parsing but before planning to catch semantic
//! errors that the grammar allows but the spec forbids. Each check maps to one
//! or more openCypher TCK error codes.
//!
//! ## Error codes covered
//!
//! | TCK error code               | Where validated                              |
//! |------------------------------|----------------------------------------------|
//! | `VariableAlreadyBound`       | CREATE/MERGE patterns, including `{}` maps   |
//! | `UndefinedVariable`          | RETURN items, SET values, WHERE, ORDER BY     |
//! | `InvalidDelete`              | DELETE with label predicate (`n:Label`)      |
//! | `InvalidAggregation`         | WHERE clause, list comprehension body        |
//! | `AmbiguousAggregationExpression` | ORDER BY mixing agg and non-agg          |
//! | `NoVariablesInScope`         | `RETURN *` with empty variable scope         |
//! | `InvalidArgumentType`        | `type(node)`, `length(node/rel)`, `size(path)`, property on path |
//! | `NonConstantExpression`      | Non-deterministic functions inside agg       |
//! | `UnexpectedSyntax`           | PatternExpr in RETURN/WITH projections       |
//! | `FloatingPointOverflow`      | Float literals that exceed f64 range         |
//! | `InvalidClauseComposition`   | Write clauses inside `EXISTS {}` subquery    |
//! | `InvalidArgumentType`        | Standalone node/rel variable as WHERE predicate |

mod expression;
mod helpers;
mod pattern_check;
mod scope;
mod semantic;
mod skip_limit;
mod vars_check;

use std::collections::HashMap;

use crate::ast::{Clause, Query};
use crate::error::GqlError;

pub(in crate::planner::validator) use self::helpers::{
    kind_name, op_name, plan_error_semantic, syntax_error,
};

/// The kind of a variable in scope — used to detect type conflicts.
///
/// Affects whether re-use in MATCH patterns is allowed:
/// - `Node` → cannot be used as a Relationship or Path
/// - `Relationship` → cannot be used as a Node or Path
/// - `Path` → cannot be used as a Node or Relationship; property access is invalid
/// - `Value` → came from a scalar expression; cannot be used as a Node in MATCH
///   (e.g. `WITH 1 AS n MATCH (n)` is a compile-time error)
/// - `Untyped` → came from CALL YIELD or UNWIND; type is unknown at compile time,
///   no conflicts are raised (the runtime determines the actual type)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VarKind {
    Node,
    Relationship,
    Path,
    Value,   // scalar from WITH literal expression (e.g. `WITH 1 AS n`)
    Untyped, // from CALL YIELD or UNWIND — type not known at compile time
}

/// Validate `query` for semantic correctness.
///
/// Returns `Err(GqlError::Parse(..))` or `Err(GqlError::Plan(..))` for
/// problems that the openCypher TCK expects to be caught at compile time.
pub fn validate_query(query: &Query) -> Result<(), GqlError> {
    let mut ctx = ValidationContext::new();
    ctx.validate(query)
}

#[derive(Clone)]
struct ValidationContext {
    /// Variables currently in scope with their kind.
    vars: HashMap<String, VarKind>,
    /// Whether we've seen a reading clause in the current segment.
    has_read: bool,
    /// Whether we've seen a writing clause in the current segment.
    has_write: bool,
    /// Count of RETURN clauses seen.
    return_count: usize,
}

impl ValidationContext {
    fn new() -> Self {
        Self {
            vars: HashMap::new(),
            has_read: false,
            has_write: false,
            return_count: 0,
        }
    }

    fn validate(&mut self, query: &Query) -> Result<(), GqlError> {
        // Validate UNION consistency
        if !query.unions.is_empty() {
            self.validate_unions(query)?;
        }

        for clause in &query.clauses {
            match clause {
                Clause::Match(mc) => {
                    self.has_read = true;
                    self.validate_match(mc)?;
                }
                Clause::OptionalMatch(mc) => {
                    self.has_read = true;
                    self.validate_match(mc)?;
                }
                Clause::Where(expr) => {
                    self.validate_where_expr(expr)?;
                }
                Clause::Return(rc) => {
                    self.return_count += 1;
                    if self.return_count > 1 {
                        return Err(syntax_error("multiple RETURN clauses in the same query"));
                    }
                    self.validate_return(rc)?;
                }
                Clause::Create(cc) => {
                    self.has_write = true;
                    self.validate_create(cc)?;
                }
                Clause::Delete(dc) => {
                    self.has_write = true;
                    self.validate_delete(dc)?;
                }
                Clause::With(wc) => {
                    self.validate_with(wc)?;
                    self.reset_scope_for_with(wc);
                }
                Clause::Unwind(uc) => {
                    self.validate_unwind(uc)?;
                }
                Clause::Set(sc) => {
                    self.has_write = true;
                    self.validate_set(sc)?;
                }
                Clause::Remove(rc) => {
                    self.has_write = true;
                    self.validate_remove(rc)?;
                }
                Clause::Merge(mc) => {
                    self.has_write = true;
                    self.validate_merge(mc)?;
                }
                Clause::Call(cc) => {
                    // CALL clauses are validated at the planner level.
                    // Register YIELD variables into scope so subsequent RETURN can find them.
                    // Use Untyped kind to avoid false conflicts with subsequent MATCH patterns.
                    for (name, alias) in &cc.yield_items {
                        let var_name = if let Some(ref a) = alias {
                            a.0.clone()
                        } else {
                            name.0.clone()
                        };
                        self.vars.insert(var_name, VarKind::Untyped);
                    }
                }
                Clause::CallSubquery { inner } => {
                    // Validate the inner subquery with a fresh context that inherits
                    // the outer scope variables (for correlated subqueries via WITH import).
                    let mut inner_ctx = ValidationContext::new();
                    inner_ctx.vars = self.vars.clone();
                    let inner_query = crate::ast::Query::new(inner.clone());
                    inner_ctx.validate(&inner_query)?;
                    // After the subquery, variables from the inner RETURN are in scope.
                    // Register them as Untyped since their actual types depend on runtime.
                    for clause in inner {
                        if let Clause::Return(rc) = clause {
                            for item in &rc.items {
                                if let Some(ref alias) = item.alias {
                                    self.vars.insert(alias.0.clone(), VarKind::Untyped);
                                } else if let crate::ast::Expr::Variable(ref id) = item.expr {
                                    self.vars.insert(id.0.clone(), VarKind::Untyped);
                                }
                            }
                        }
                    }
                }
                Clause::Foreach(_) => {
                    // FOREACH clauses — validated at the planner level.
                }
                Clause::LoadCsv(lc) => {
                    self.has_read = true;
                    // Register the alias variable so subsequent clauses can reference it.
                    self.vars.insert(lc.alias.0.clone(), VarKind::Untyped);
                }
            }
        }
        Ok(())
    }

    // ── Helper inherent methods live in sibling files: ─────────────────────
    // - vars_check.rs    : check_vars_defined
    // - pattern_check.rs : check_no_pattern_expr
    // - skip_limit.rs    : validate_skip_limit
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_query;

    fn validate(query: &str) -> Result<(), GqlError> {
        let ast = parse_query(query).unwrap();
        validate_query(&ast)
    }

    fn expect_error(query: &str) {
        assert!(
            validate(query).is_err(),
            "Expected validation error for: {query}"
        );
    }

    fn expect_ok(query: &str) {
        assert!(
            validate(query).is_ok(),
            "Expected validation to pass for: {query}"
        );
    }

    #[test]
    fn valid_simple_match_return() {
        expect_ok("MATCH (n) RETURN n");
    }

    #[test]
    fn valid_create_with_type() {
        expect_ok("CREATE (a)-[:KNOWS]->(b)");
    }

    #[test]
    fn create_without_rel_type_errors() {
        let result = crate::parser::parse_query("CREATE ()-[]->(b)");
        if let Ok(ast) = result {
            assert!(validate_query(&ast).is_err());
        }
    }

    #[test]
    fn create_var_length_errors() {
        let result = crate::parser::parse_query("CREATE (a)-[:FOO*2]->(b)");
        if let Ok(ast) = result {
            assert!(validate_query(&ast).is_err());
        }
    }

    #[test]
    fn negative_skip_errors() {
        expect_error("MATCH (n) RETURN n SKIP -1");
    }

    #[test]
    fn float_limit_errors() {
        expect_error("MATCH (n) RETURN n LIMIT 1.5");
    }

    #[test]
    fn column_name_conflict_errors() {
        expect_error("RETURN 1 AS a, 2 AS a");
    }

    #[test]
    fn valid_with_clause() {
        expect_ok("MATCH (n) WITH n RETURN n");
    }

    #[test]
    fn not_on_integer_errors() {
        expect_error("RETURN NOT 0");
    }

    #[test]
    fn not_on_string_errors() {
        expect_error("RETURN NOT 'hello'");
    }

    #[test]
    fn and_on_integer_errors() {
        expect_error("RETURN 0 AND true");
    }

    #[test]
    fn or_on_string_errors() {
        expect_error("RETURN 'a' OR false");
    }

    #[test]
    fn valid_boolean_operations() {
        expect_ok("RETURN true AND false");
        expect_ok("RETURN NOT true");
    }

    #[test]
    fn variable_type_conflict_rel_as_node() {
        expect_error("MATCH ()-[r]-() MATCH (r) RETURN r");
    }

    #[test]
    fn variable_type_conflict_node_as_rel() {
        expect_error("MATCH (n) MATCH ()-[n]-() RETURN n");
    }
}
