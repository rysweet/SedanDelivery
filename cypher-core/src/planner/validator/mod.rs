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
mod scope;
mod semantic;

use std::collections::HashMap;

use crate::ast::{BinaryOp, Clause, Expr, Query, UnaryOp};
use crate::error::{GqlError, ParseError, PlanError};

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

    // ── Undefined variable checking ─────────────────────────────────────────

    /// Check that all variable references in an expression are defined in scope.
    /// `extra_vars` provides additional in-scope variables (e.g. list comprehension loop vars).
    fn check_vars_defined(
        &self,
        expr: &Expr,
        extra_vars: &std::collections::HashSet<String>,
    ) -> Result<(), GqlError> {
        match expr {
            Expr::Variable(ref id) => {
                if id.0 != "*" && !self.vars.contains_key(&id.0) && !extra_vars.contains(&id.0) {
                    return Err(plan_error_semantic(&format!(
                        "variable '{}' not defined",
                        id.0
                    )));
                }
                Ok(())
            }
            Expr::Property(inner, _) => self.check_vars_defined(inner, extra_vars),
            Expr::BinaryOp { left, right, .. } => {
                self.check_vars_defined(left, extra_vars)?;
                self.check_vars_defined(right, extra_vars)
            }
            Expr::UnaryOp { expr, .. } => self.check_vars_defined(expr, extra_vars),
            Expr::FunctionCall { name, args, .. } => {
                // Pattern comprehensions introduce their own variable scope — skip var checking
                if name.as_str() == "__patternComp" {
                    return Ok(());
                }
                for arg in args {
                    self.check_vars_defined(arg, extra_vars)?;
                }
                Ok(())
            }
            Expr::IsNull(inner) | Expr::IsNotNull(inner) | Expr::HasLabel(inner, _) => {
                self.check_vars_defined(inner, extra_vars)
            }
            Expr::ListComp {
                variable,
                list,
                filter,
                map,
                ..
            } => {
                self.check_vars_defined(list, extra_vars)?;
                // Add the loop variable to scope for filter and map
                let mut inner_extra = extra_vars.clone();
                inner_extra.insert(variable.0.clone());
                if let Some(f) = filter {
                    self.check_vars_defined(f, &inner_extra)?;
                }
                self.check_vars_defined(map, &inner_extra)
            }
            Expr::Reduce {
                accumulator,
                initial,
                variable,
                list,
                expression,
            } => {
                self.check_vars_defined(initial, extra_vars)?;
                self.check_vars_defined(list, extra_vars)?;
                // Both accumulator and variable are in scope for the expression
                let mut inner_extra = extra_vars.clone();
                inner_extra.insert(accumulator.0.clone());
                inner_extra.insert(variable.0.clone());
                self.check_vars_defined(expression, &inner_extra)
            }
            Expr::List(items) => {
                for item in items {
                    self.check_vars_defined(item, extra_vars)?;
                }
                Ok(())
            }
            Expr::MapLiteral(pairs) => {
                for (_, v) in pairs {
                    self.check_vars_defined(v, extra_vars)?;
                }
                Ok(())
            }
            Expr::Case {
                subject,
                alternatives,
                default,
            } => {
                if let Some(s) = subject {
                    self.check_vars_defined(s, extra_vars)?;
                }
                for (w, t) in alternatives {
                    self.check_vars_defined(w, extra_vars)?;
                    self.check_vars_defined(t, extra_vars)?;
                }
                if let Some(d) = default {
                    self.check_vars_defined(d, extra_vars)?;
                }
                Ok(())
            }
            Expr::Subscript(a, b) => {
                self.check_vars_defined(a, extra_vars)?;
                self.check_vars_defined(b, extra_vars)
            }
            Expr::Slice(a, b, c) => {
                self.check_vars_defined(a, extra_vars)?;
                if let Some(b) = b {
                    self.check_vars_defined(b, extra_vars)?;
                }
                if let Some(c) = c {
                    self.check_vars_defined(c, extra_vars)?;
                }
                Ok(())
            }
            // PatternExpr and ExistsSubquery introduce their own scope — skip
            Expr::PatternExpr(_) | Expr::ExistsSubquery { .. } => Ok(()),
            _ => Ok(()),
        }
    }

    // ── PatternExpr placement check ─────────────────────────────────────────

    /// Check that no PatternExpr appears in a non-WHERE context (UnexpectedSyntax).
    /// PatternExpr is allowed as the source of a `__patterncomp` (pattern comprehension).
    fn check_no_pattern_expr(&self, expr: &Expr) -> Result<(), GqlError> {
        match expr {
            Expr::PatternExpr(_) => Err(plan_error_semantic(
                "pattern expressions are not allowed in RETURN or WITH projections",
            )),
            Expr::Property(inner, _) => self.check_no_pattern_expr(inner),
            Expr::BinaryOp { left, right, .. } => {
                self.check_no_pattern_expr(left)?;
                self.check_no_pattern_expr(right)
            }
            Expr::UnaryOp { expr, .. } => self.check_no_pattern_expr(expr),
            Expr::FunctionCall { name, args, .. } => {
                // __patternComp is a pattern comprehension — its first arg is a PatternExpr
                // and that's valid; only check the remaining args (the projection expression)
                if name.as_str() == "__patternComp" {
                    // Skip the first arg (PatternExpr source), check the rest
                    for arg in args.iter().skip(1) {
                        self.check_no_pattern_expr(arg)?;
                    }
                } else {
                    for arg in args {
                        self.check_no_pattern_expr(arg)?;
                    }
                }
                Ok(())
            }
            Expr::IsNull(inner) | Expr::IsNotNull(inner) | Expr::HasLabel(inner, _) => {
                self.check_no_pattern_expr(inner)
            }
            Expr::List(items) => {
                for item in items {
                    self.check_no_pattern_expr(item)?;
                }
                Ok(())
            }
            Expr::MapLiteral(pairs) => {
                for (_, v) in pairs {
                    self.check_no_pattern_expr(v)?;
                }
                Ok(())
            }
            Expr::ListComp {
                list, filter, map, ..
            } => {
                self.check_no_pattern_expr(list)?;
                if let Some(f) = filter {
                    self.check_no_pattern_expr(f)?;
                }
                self.check_no_pattern_expr(map)
            }
            _ => Ok(()),
        }
    }

    // ── SKIP/LIMIT validation ───────────────────────────────────────────────

    fn validate_skip_limit(&self, expr: &Expr, clause_name: &str) -> Result<(), GqlError> {
        match expr {
            Expr::Integer(n) => {
                if *n < 0 {
                    return Err(plan_error_semantic(&format!(
                        "{clause_name} requires a non-negative integer, got {n}"
                    )));
                }
            }
            Expr::Float(_) => {
                return Err(plan_error_semantic(&format!(
                    "{clause_name} requires an integer, got a floating point value"
                )));
            }
            Expr::UnaryOp {
                op: UnaryOp::Neg,
                expr: inner,
            } => match inner.as_ref() {
                Expr::Integer(_) => {
                    return Err(plan_error_semantic(&format!(
                        "{clause_name} requires a non-negative integer"
                    )));
                }
                Expr::Float(_) => {
                    return Err(plan_error_semantic(&format!(
                        "{clause_name} requires an integer, got a floating point value"
                    )));
                }
                _ => {}
            },
            Expr::Str(_) | Expr::Bool(_) | Expr::Null | Expr::List(_) | Expr::MapLiteral(_) => {
                return Err(plan_error_semantic(&format!(
                    "{clause_name} requires an integer expression"
                )));
            }
            _ => {}
        }
        Ok(())
    }
}

// ── Helper functions ────────────────────────────────────────────────────────

fn kind_name(k: VarKind) -> &'static str {
    match k {
        VarKind::Node => "Node",
        VarKind::Relationship => "Relationship",
        VarKind::Path => "Path",
        VarKind::Value => "Value",
        VarKind::Untyped => "Untyped",
    }
}

fn op_name(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::And => "AND",
        BinaryOp::Or => "OR",
        BinaryOp::Xor => "XOR",
        BinaryOp::Add | BinaryOp::Concat => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::Pow => "^",
        BinaryOp::StartsWith => "STARTS WITH",
        BinaryOp::EndsWith => "ENDS WITH",
        BinaryOp::Contains => "CONTAINS",
        BinaryOp::RegexMatch => "=~",
        _ => "operator",
    }
}

// ── Error constructors ──────────────────────────────────────────────────────

fn plan_error_semantic(msg: &str) -> GqlError {
    GqlError::Plan(PlanError::SemanticError {
        message: msg.to_string(),
    })
}

fn syntax_error(msg: &str) -> GqlError {
    GqlError::Parse(ParseError::Syntax {
        offset: 0,
        expected: msg.to_string(),
        found: String::new(),
    })
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
