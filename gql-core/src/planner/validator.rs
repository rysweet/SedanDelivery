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

use std::collections::{HashMap, HashSet};

use crate::ast::{
    BinaryOp, Clause, CreateClause, DeleteClause, Direction, Expr, MatchClause, MergeClause,
    NodePattern, PathPattern, Query, RelPattern, RemoveClause, ReturnClause, ReturnItem, SetClause,
    UnaryOp, UnwindClause, WithClause,
};
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
                Clause::Foreach(_) => {
                    // FOREACH clauses — validated at the planner level.
                }
            }
        }
        Ok(())
    }

    // ── MATCH validation ────────────────────────────────────────────────────

    fn validate_match(&mut self, mc: &MatchClause) -> Result<(), GqlError> {
        for pattern in &mc.patterns {
            self.validate_match_pattern(pattern)?;
        }
        if let Some(pred) = &mc.where_clause {
            self.validate_where_expr(pred)?;
        }
        Ok(())
    }

    fn validate_match_pattern(&mut self, pattern: &PathPattern) -> Result<(), GqlError> {
        // Path variable binding
        if let Some(ref path_var) = pattern.variable {
            let name = path_var.0.clone();
            if let Some(&existing_kind) = self.vars.get(&name) {
                if existing_kind != VarKind::Path {
                    return Err(plan_error_syntax(&format!(
                        "variable '{name}' already defined as {}, cannot rebind as Path",
                        kind_name(existing_kind)
                    )));
                }
            }
            self.vars.insert(name, VarKind::Path);
        }

        self.validate_node_pattern_in_match(&pattern.start)?;

        // Track relationship variables within this single pattern for uniqueness
        let mut pattern_rel_vars: HashSet<String> = HashSet::new();

        for step in &pattern.steps {
            // Check relationship uniqueness within the same pattern
            if let Some(ref var) = step.rel.variable {
                if !pattern_rel_vars.insert(var.0.clone()) {
                    return Err(plan_error_syntax(&format!(
                        "relationship variable '{}' used multiple times in the same MATCH pattern",
                        var.0
                    )));
                }
            }
            self.validate_rel_pattern_in_match(&step.rel)?;
            self.validate_node_pattern_in_match(&step.node)?;
        }
        Ok(())
    }

    fn validate_node_pattern_in_match(&mut self, node: &NodePattern) -> Result<(), GqlError> {
        if let Some(ref var) = node.variable {
            let name = var.0.clone();
            if let Some(&existing) = self.vars.get(&name) {
                if existing == VarKind::Relationship || existing == VarKind::Path {
                    return Err(plan_error_syntax(&format!(
                        "variable '{name}' already defined as {}, cannot be used as Node",
                        kind_name(existing)
                    )));
                }
                // Value -> Node: e.g. `WITH true AS n MATCH (n)` is a type conflict
                // because a literal value cannot be used as a node pattern.
                // Untyped (from CALL YIELD or UNWIND) can become a Node — no conflict.
                if existing == VarKind::Value {
                    return Err(plan_error_syntax(&format!(
                        "variable '{name}' already defined as Value, cannot be used as Node",
                    )));
                }
            }
            self.vars.insert(name, VarKind::Node);
        }
        if let Some(props) = &node.properties {
            for (_, expr) in props {
                self.validate_expr_general(expr)?;
            }
        }
        Ok(())
    }

    fn validate_rel_pattern_in_match(&mut self, rel: &RelPattern) -> Result<(), GqlError> {
        if let Some(ref var) = rel.variable {
            let name = var.0.clone();
            if let Some(&existing) = self.vars.get(&name) {
                if existing == VarKind::Node || existing == VarKind::Path {
                    return Err(plan_error_syntax(&format!(
                        "variable '{name}' already defined as {}, cannot be used as Relationship",
                        kind_name(existing)
                    )));
                }
                // Value -> Relationship: e.g. `WITH true AS r MATCH ()-[r]-()` is a type conflict
                // Exception: variable-length rel patterns like `-[rs*]->` can use a list variable
                // Untyped (from CALL YIELD) can become a Relationship — no conflict.
                if existing == VarKind::Value && rel.range.is_none() {
                    return Err(plan_error_syntax(&format!(
                        "variable '{name}' already defined as Value, cannot be used as Relationship",
                    )));
                }
            }
            self.vars.insert(name, VarKind::Relationship);
        }
        if let Some(props) = &rel.properties {
            for (_, expr) in props {
                self.validate_expr_general(expr)?;
            }
        }
        Ok(())
    }

    // ── WHERE expression validation ─────────────────────────────────────────

    fn validate_where_expr(&self, expr: &Expr) -> Result<(), GqlError> {
        // Aggregation functions are not allowed in WHERE clauses (InvalidAggregation)
        if contains_aggregation(expr) {
            return Err(plan_error_syntax(
                "aggregation functions are not allowed in WHERE clauses",
            ));
        }
        // A standalone node/relationship variable used as a boolean predicate is
        // InvalidArgumentType — e.g. `WHERE n` where n is a node
        if let Expr::Variable(ref id) = expr {
            if let Some(&kind) = self.vars.get(&id.0) {
                if kind == VarKind::Node || kind == VarKind::Relationship {
                    return Err(plan_error_syntax(&format!(
                        "variable '{}' is a {} and cannot be used as a boolean predicate",
                        id.0,
                        kind_name(kind)
                    )));
                }
            }
        }
        // Check PatternExpr in WHERE: named variables must be in scope
        self.check_where_pattern_vars(expr)?;
        // Check that all variable references are defined
        self.check_vars_defined_in_where(expr)?;
        self.validate_expr_general(expr)
    }

    /// Check that all variable references in a WHERE expression are defined in scope.
    /// PatternExpr variables are checked separately by check_where_pattern_vars.
    fn check_vars_defined_in_where(&self, expr: &Expr) -> Result<(), GqlError> {
        match expr {
            // PatternExpr in WHERE — variable scope is checked by check_where_pattern_vars
            Expr::PatternExpr(_) | Expr::ExistsSubquery { .. } => Ok(()),
            Expr::Variable(ref id) => {
                if id.0 != "*" && !self.vars.contains_key(&id.0) {
                    return Err(plan_error_syntax(&format!(
                        "variable '{}' not defined",
                        id.0
                    )));
                }
                Ok(())
            }
            Expr::Property(inner, _) => {
                // Property access on a Path variable is invalid (InvalidArgumentType)
                if let Expr::Variable(ref id) = inner.as_ref() {
                    if let Some(&kind) = self.vars.get(&id.0) {
                        if kind == VarKind::Path {
                            return Err(plan_error_syntax(&format!(
                                "property access on path variable '{}' is not allowed",
                                id.0
                            )));
                        }
                    }
                }
                self.check_vars_defined_in_where(inner)
            }
            Expr::BinaryOp { left, right, .. } => {
                self.check_vars_defined_in_where(left)?;
                self.check_vars_defined_in_where(right)
            }
            Expr::UnaryOp { expr, .. } => self.check_vars_defined_in_where(expr),
            Expr::FunctionCall { name, args, .. } => {
                // Pattern comprehensions introduce their own variable scope — skip var checking
                if name.as_str() == "__patternComp" {
                    return Ok(());
                }
                for arg in args {
                    self.check_vars_defined_in_where(arg)?;
                }
                Ok(())
            }
            Expr::IsNull(inner) | Expr::IsNotNull(inner) | Expr::HasLabel(inner, _) => {
                self.check_vars_defined_in_where(inner)
            }
            Expr::ListComp {
                variable,
                list,
                filter,
                map,
                ..
            } => {
                self.check_vars_defined_in_where(list)?;
                // loop variable is valid inside filter and map
                let mut inner_vars = self.vars.clone();
                inner_vars.insert(variable.0.clone(), VarKind::Value);
                let inner_ctx = ValidationContext {
                    vars: inner_vars,
                    has_read: self.has_read,
                    has_write: self.has_write,
                    return_count: self.return_count,
                };
                if let Some(f) = filter {
                    inner_ctx.check_vars_defined_in_where(f)?;
                }
                inner_ctx.check_vars_defined_in_where(map)
            }
            Expr::List(items) => {
                for item in items {
                    self.check_vars_defined_in_where(item)?;
                }
                Ok(())
            }
            Expr::MapLiteral(pairs) => {
                for (_, v) in pairs {
                    self.check_vars_defined_in_where(v)?;
                }
                Ok(())
            }
            Expr::Case {
                subject,
                alternatives,
                default,
            } => {
                if let Some(s) = subject {
                    self.check_vars_defined_in_where(s)?;
                }
                for (w, t) in alternatives {
                    self.check_vars_defined_in_where(w)?;
                    self.check_vars_defined_in_where(t)?;
                }
                if let Some(d) = default {
                    self.check_vars_defined_in_where(d)?;
                }
                Ok(())
            }
            Expr::Subscript(a, b) => {
                self.check_vars_defined_in_where(a)?;
                self.check_vars_defined_in_where(b)
            }
            Expr::Slice(a, b, c) => {
                self.check_vars_defined_in_where(a)?;
                if let Some(b) = b {
                    self.check_vars_defined_in_where(b)?;
                }
                if let Some(c) = c {
                    self.check_vars_defined_in_where(c)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Validate PatternExpr in WHERE: all named variables in the pattern must be in scope.
    /// Also detects standalone node patterns (no relationships) as InvalidArgumentType.
    fn check_where_pattern_vars(&self, expr: &Expr) -> Result<(), GqlError> {
        match expr {
            Expr::PatternExpr(patterns) => {
                for pattern in patterns {
                    // Standalone node-only pattern (no relationships) is InvalidArgumentType
                    if pattern.steps.is_empty() {
                        if let Some(ref var) = pattern.start.variable {
                            // (n) with no relationships — checks if node exists but doesn't test a predicate
                            if self.vars.contains_key(&var.0) {
                                return Err(plan_error_syntax(
                                    "standalone node pattern in WHERE is not a valid predicate",
                                ));
                            }
                        }
                    }
                    // All named variables in the pattern must be in scope
                    self.check_pattern_vars_in_scope(pattern)?;
                }
                Ok(())
            }
            Expr::BinaryOp { left, right, .. } => {
                self.check_where_pattern_vars(left)?;
                self.check_where_pattern_vars(right)
            }
            Expr::UnaryOp { expr, .. } => self.check_where_pattern_vars(expr),
            Expr::IsNull(inner) | Expr::IsNotNull(inner) | Expr::HasLabel(inner, _) => {
                self.check_where_pattern_vars(inner)
            }
            _ => Ok(()),
        }
    }

    /// Check that all named variables in a path pattern are already in scope.
    fn check_pattern_vars_in_scope(&self, pattern: &PathPattern) -> Result<(), GqlError> {
        if let Some(ref var) = pattern.start.variable {
            if !var.0.is_empty() && !self.vars.contains_key(&var.0) {
                return Err(plan_error_syntax(&format!(
                    "variable '{}' not defined",
                    var.0
                )));
            }
        }
        for step in &pattern.steps {
            if let Some(ref var) = step.rel.variable {
                if !var.0.is_empty() && !self.vars.contains_key(&var.0) {
                    return Err(plan_error_syntax(&format!(
                        "variable '{}' not defined",
                        var.0
                    )));
                }
            }
            if let Some(ref var) = step.node.variable {
                if !var.0.is_empty() && !self.vars.contains_key(&var.0) {
                    return Err(plan_error_syntax(&format!(
                        "variable '{}' not defined",
                        var.0
                    )));
                }
            }
        }
        Ok(())
    }

    // ── CREATE validation ───────────────────────────────────────────────────

    fn validate_create(&mut self, cc: &CreateClause) -> Result<(), GqlError> {
        for pattern in &cc.patterns {
            self.validate_create_pattern(pattern)?;
        }
        Ok(())
    }

    fn validate_create_pattern(&mut self, pattern: &PathPattern) -> Result<(), GqlError> {
        // Standalone `CREATE (a)` where `a` is already bound is VariableAlreadyBound
        if pattern.steps.is_empty() {
            if let Some(ref var) = pattern.start.variable {
                if self.vars.contains_key(&var.0) {
                    return Err(plan_error_syntax(&format!(
                        "variable '{}' already bound, cannot create standalone node",
                        var.0
                    )));
                }
            }
        }
        self.validate_node_in_create(&pattern.start)?;
        for step in &pattern.steps {
            self.validate_rel_in_create(&step.rel)?;
            self.validate_node_in_create(&step.node)?;
        }
        Ok(())
    }

    fn validate_node_in_create(&mut self, node: &NodePattern) -> Result<(), GqlError> {
        if let Some(ref var) = node.variable {
            let name = var.0.clone();
            if self.vars.contains_key(&name) {
                let has_labels = !node.labels.is_empty();
                // Any explicit property map (even empty {}) is a rebinding attempt
                let has_props = node.properties.is_some();
                // VariableAlreadyBound: re-use with labels or any property map
                if has_labels || has_props {
                    return Err(plan_error_syntax(&format!(
                        "variable '{name}' already bound, cannot be rebound in CREATE with labels/properties"
                    )));
                }
                // Note: bare reference like (a) in `CREATE (a)-[:KNOWS]->(b)` is OK
                // but standalone `CREATE (a)` is caught separately
            }
            self.vars.insert(name, VarKind::Node);
        }
        if let Some(props) = &node.properties {
            for (_, expr) in props {
                self.validate_expr_in_create(expr)?;
            }
        }
        Ok(())
    }

    fn validate_rel_in_create(&mut self, rel: &RelPattern) -> Result<(), GqlError> {
        if rel.rel_types.is_empty() {
            return Err(plan_error_syntax(
                "CREATE requires exactly one relationship type",
            ));
        }
        if rel.rel_types.len() > 1 {
            return Err(plan_error_syntax(
                "CREATE requires exactly one relationship type, got multiple",
            ));
        }
        if rel.direction == Direction::Undirected {
            return Err(plan_error_syntax(
                "CREATE requires a directed relationship pattern",
            ));
        }
        if rel.range.is_some() {
            return Err(plan_error_syntax(
                "variable-length relationship patterns are not allowed in CREATE",
            ));
        }
        if let Some(ref var) = rel.variable {
            let name = var.0.clone();
            if self.vars.contains_key(&name) {
                return Err(plan_error_syntax(&format!(
                    "variable '{name}' already bound, cannot be rebound in CREATE"
                )));
            }
            self.vars.insert(name, VarKind::Relationship);
        }
        if let Some(props) = &rel.properties {
            for (_, expr) in props {
                self.validate_expr_in_create(expr)?;
            }
        }
        Ok(())
    }

    fn validate_expr_in_create(&self, expr: &Expr) -> Result<(), GqlError> {
        match expr {
            Expr::Variable(ref id) => {
                if !self.vars.contains_key(&id.0) {
                    return Err(plan_error_syntax(&format!(
                        "variable '{}' not defined",
                        id.0
                    )));
                }
                Ok(())
            }
            Expr::Property(inner, _) => self.validate_expr_in_create(inner),
            Expr::BinaryOp { left, right, .. } => {
                self.validate_expr_in_create(left)?;
                self.validate_expr_in_create(right)
            }
            Expr::UnaryOp { expr, .. } => self.validate_expr_in_create(expr),
            Expr::FunctionCall { args, .. } => {
                for arg in args {
                    self.validate_expr_in_create(arg)?;
                }
                Ok(())
            }
            Expr::List(items) => {
                for item in items {
                    self.validate_expr_in_create(item)?;
                }
                Ok(())
            }
            Expr::MapLiteral(pairs) => {
                for (_, v) in pairs {
                    self.validate_expr_in_create(v)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    // ── DELETE validation ────────────────────────────────────────────────────

    fn validate_delete(&self, dc: &DeleteClause) -> Result<(), GqlError> {
        for expr in &dc.exprs {
            self.validate_delete_expr(expr)?;
        }
        Ok(())
    }

    fn validate_delete_expr(&self, expr: &Expr) -> Result<(), GqlError> {
        match expr {
            Expr::Variable(ref id) => {
                if !self.vars.contains_key(&id.0) {
                    return Err(plan_error_syntax(&format!(
                        "variable '{}' not defined",
                        id.0
                    )));
                }
                Ok(())
            }
            // DELETE n.prop, DELETE map.key
            Expr::Property(inner, _) => self.validate_delete_expr(inner),
            // DELETE list[idx]
            Expr::Subscript(inner, _) => self.validate_delete_expr(inner),
            // DELETE n:Label is invalid — label predicates are not valid in DELETE
            Expr::HasLabel(_, _) => {
                Err(plan_error_syntax(
                    "invalid use of a label predicate in DELETE; use REMOVE to remove labels",
                ))
            }
            // Literal values in DELETE are invalid: DELETE 1+1
            Expr::Integer(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Null => {
                Err(plan_error_syntax(
                    "DELETE requires a variable reference, got a literal",
                ))
            }
            // Arithmetic expressions on literals: DELETE 1+1
            Expr::BinaryOp { left, right, .. } => {
                if is_non_variable_expr(left) && is_non_variable_expr(right) {
                    return Err(plan_error_syntax(
                        "DELETE requires a variable reference, got an expression",
                    ));
                }
                Ok(())
            }
            // Other expressions (function calls, etc.) — allow through for runtime checking
            _ => Ok(()),
        }
    }

    // ── RETURN validation ───────────────────────────────────────────────────

    fn validate_return(&self, rc: &ReturnClause) -> Result<(), GqlError> {
        // Check RETURN * with empty scope — `RETURN *` is represented as Variable("*")
        let is_return_star = rc.items.len() == 1
            && matches!(&rc.items[0].expr, Expr::Variable(ref id) if id.0 == "*");
        if is_return_star && self.vars.is_empty() {
            return Err(plan_error_syntax(
                "RETURN * requires at least one variable in scope",
            ));
        }
        // Check for undefined variables in RETURN (only top-level variables, not inside expressions)
        if !is_return_star && self.has_read {
            for item in &rc.items {
                if let Expr::Variable(ref id) = item.expr {
                    if id.0 != "*" && !self.vars.contains_key(&id.0) {
                        return Err(plan_error_syntax(&format!(
                            "variable '{}' not defined",
                            id.0
                        )));
                    }
                }
            }
        }
        self.check_column_name_conflicts(&rc.items)?;
        self.validate_aggregation_consistency(&rc.items)?;
        // Validate RETURN expressions
        for item in &rc.items {
            self.validate_expr_general(&item.expr)?;
            // Check for PatternExpr in RETURN projection (UnexpectedSyntax)
            self.check_no_pattern_expr(&item.expr)?;
            // Check that all variable references are defined (UndefinedVariable)
            if !is_return_star {
                self.check_vars_defined(&item.expr, &std::collections::HashSet::new())?;
            }
        }
        // Validate ORDER BY — check for aggregation in ORDER BY
        let has_agg_in_return = rc.items.iter().any(|item| contains_aggregation(&item.expr));

        // Build the set of names available after RETURN DISTINCT (projected names only)
        let return_projected_names: HashSet<String> = rc
            .items
            .iter()
            .filter_map(|item| {
                if let Some(ref alias) = item.alias {
                    Some(alias.0.clone())
                } else if let Expr::Variable(ref id) = item.expr {
                    Some(id.0.clone())
                } else {
                    None
                }
            })
            .collect();

        for sort_item in &rc.order_by {
            if !has_agg_in_return && contains_aggregation(&sort_item.expr) {
                return Err(plan_error_syntax(
                    "aggregation in ORDER BY requires aggregation in RETURN",
                ));
            }
            // After RETURN DISTINCT, only projected names are in scope for ORDER BY
            if rc.distinct && has_agg_in_return {
                // With aggregation + DISTINCT, strictly check ORDER BY scope
                self.validate_order_by_expr_scope(&sort_item.expr, &return_projected_names)?;
            } else if rc.distinct {
                // DISTINCT without aggregation: ORDER BY must use projected names only
                // (variables not projected are no longer in scope)
                self.validate_order_by_expr_scope(&sort_item.expr, &return_projected_names)?;
            }
            // Check for ambiguous aggregation in ORDER BY when RETURN has aggregation
            if has_agg_in_return && has_ambiguous_aggregation(&sort_item.expr) {
                return Err(plan_error_syntax(
                    "ambiguous aggregation in ORDER BY: expression mixes aggregation and non-aggregation",
                ));
            }
            self.validate_order_by_expr(&sort_item.expr, &rc.items)?;
        }
        if let Some(ref skip) = rc.skip {
            self.validate_skip_limit(skip, "SKIP")?;
        }
        if let Some(ref limit) = rc.limit {
            self.validate_skip_limit(limit, "LIMIT")?;
        }
        Ok(())
    }

    /// Validate that ORDER BY expression only references projected names.
    fn validate_order_by_expr_scope(
        &self,
        expr: &Expr,
        projected_names: &HashSet<String>,
    ) -> Result<(), GqlError> {
        match expr {
            Expr::Variable(ref id) => {
                if id.0 != "*" && !projected_names.contains(&id.0) && !self.vars.contains_key(&id.0) {
                    return Err(plan_error_syntax(&format!(
                        "variable '{}' not in scope after RETURN/WITH DISTINCT",
                        id.0
                    )));
                }
            }
            Expr::Property(inner, _) => {
                // For property access like `a.age`, check if `a` itself is projected
                // After DISTINCT, if `a` is not projected but only `a.name` is, then `a.age` is undefined
                if let Expr::Variable(ref id) = inner.as_ref() {
                    // Check if the variable `a` is directly projected OR if any property of `a` is projected
                    if !projected_names.contains(&id.0) && !self.vars.contains_key(&id.0) {
                        return Err(plan_error_syntax(&format!(
                            "variable '{}' not in scope after RETURN/WITH DISTINCT",
                            id.0
                        )));
                    }
                    // Check if the node variable `a` is projected (not just a.name property)
                    // When DISTINCT is used and only a.name was projected (not `a`), then a.age is invalid
                    if !projected_names.contains(&id.0) {
                        // Only the property `a.name` was projected, not `a` itself
                        // `a.age` would reference the original variable which is no longer in scope
                        return Err(plan_error_syntax(&format!(
                            "variable '{}' not projected by DISTINCT",
                            id.0
                        )));
                    }
                }
            }
            Expr::BinaryOp { left, right, .. } => {
                self.validate_order_by_expr_scope(left, projected_names)?;
                self.validate_order_by_expr_scope(right, projected_names)?;
            }
            Expr::UnaryOp { expr, .. } => {
                self.validate_order_by_expr_scope(expr, projected_names)?;
            }
            Expr::FunctionCall { args, .. } => {
                for arg in args {
                    self.validate_order_by_expr_scope(arg, projected_names)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ── WITH validation ─────────────────────────────────────────────────────

    fn validate_with(&self, wc: &WithClause) -> Result<(), GqlError> {
        // WITH requires all non-variable expressions to have aliases
        for item in &wc.items {
            if item.alias.is_none() && !matches!(item.expr, Expr::Variable(_)) {
                // Expression without alias in WITH — unless it's `*`
                if matches!(&item.expr, Expr::Variable(ref id) if id.0 == "*") {
                    continue;
                }
                return Err(plan_error_syntax(
                    "expression in WITH must be aliased (use AS)",
                ));
            }
        }
        self.check_with_column_conflicts(&wc.items)?;
        self.validate_aggregation_consistency(&wc.items)?;
        for item in &wc.items {
            self.validate_expr_general(&item.expr)?;
            // Check for PatternExpr in WITH projection (UnexpectedSyntax)
            self.check_no_pattern_expr(&item.expr)?;
        }
        let with_names: HashSet<String> = wc
            .items
            .iter()
            .filter_map(|item| {
                if let Some(ref alias) = item.alias {
                    Some(alias.0.clone())
                } else if let Expr::Variable(ref id) = item.expr {
                    Some(id.0.clone())
                } else {
                    None
                }
            })
            .collect();

        // Check for aggregation in ORDER BY when WITH projection has no aggregation
        let has_agg_in_projection = wc.items.iter().any(|item| contains_aggregation(&item.expr));

        // Collect variables used in the non-aggregated (grouping key) WITH items.
        // These variables can be referenced in ORDER BY even after aggregation.
        let mut grouping_key_vars: HashSet<String> = HashSet::new();
        if has_agg_in_projection {
            for item in &wc.items {
                if !contains_aggregation(&item.expr) {
                    collect_variable_refs(&item.expr, &mut grouping_key_vars);
                }
            }
        }

        for sort_item in &wc.order_by {
            if !has_agg_in_projection && contains_aggregation(&sort_item.expr) {
                return Err(plan_error_syntax(
                    "aggregation in ORDER BY requires aggregation in WITH projection",
                ));
            }
            // Check for ambiguous aggregation in ORDER BY when WITH has aggregation
            if has_agg_in_projection && has_ambiguous_aggregation(&sort_item.expr) {
                return Err(plan_error_syntax(
                    "ambiguous aggregation in ORDER BY: expression mixes aggregation and non-aggregation",
                ));
            }
            if has_agg_in_projection {
                // When aggregation is present, ORDER BY can only reference:
                // 1. Projected names (with_names)
                // 2. Variables used in the grouping key expressions
                // (not arbitrary variables from the previous scope)
                self.validate_order_by_expr_strict_with(
                    &sort_item.expr,
                    &with_names,
                    &grouping_key_vars,
                )?;
            } else {
                self.validate_order_by_expr_in_with(&sort_item.expr, &with_names)?;
            }
        }
        // Note: WHERE in WITH is validated loosely — the executor will catch
        // truly undefined variables. We skip strict scope checking here to avoid
        // rejecting valid queries that use complex expressions.
        if let Some(ref pred) = wc.where_clause {
            self.validate_expr_general(pred)?;
        }
        if let Some(ref skip) = wc.skip {
            self.validate_skip_limit(skip, "SKIP")?;
        }
        if let Some(ref limit) = wc.limit {
            self.validate_skip_limit(limit, "LIMIT")?;
        }
        Ok(())
    }

    /// Strict ORDER BY scope check for WITH with aggregation.
    /// Variables outside aggregation calls must be from with_names or grouping_key_vars.
    /// Variables INSIDE aggregation calls are allowed (they reference pre-aggregation data).
    fn validate_order_by_expr_strict_with(
        &self,
        expr: &Expr,
        with_names: &HashSet<String>,
        grouping_key_vars: &HashSet<String>,
    ) -> Result<(), GqlError> {
        self.validate_order_by_expr_strict_with_inner(expr, with_names, grouping_key_vars, false)
    }

    fn validate_order_by_expr_strict_with_inner(
        &self,
        expr: &Expr,
        with_names: &HashSet<String>,
        grouping_key_vars: &HashSet<String>,
        inside_aggregation: bool,
    ) -> Result<(), GqlError> {
        match expr {
            Expr::Variable(ref id) => {
                // Variables inside aggregation calls are allowed (pre-agg data access)
                if !inside_aggregation
                    && id.0 != "*"
                    && !with_names.contains(&id.0)
                    && !grouping_key_vars.contains(&id.0)
                {
                    return Err(plan_error_syntax(&format!(
                        "variable '{}' not in scope after WITH projection",
                        id.0
                    )));
                }
            }
            Expr::FunctionCall { name, args, .. } => {
                let next_inside_agg =
                    inside_aggregation || is_aggregation_function(name.as_str());
                for arg in args {
                    self.validate_order_by_expr_strict_with_inner(
                        arg,
                        with_names,
                        grouping_key_vars,
                        next_inside_agg,
                    )?;
                }
            }
            Expr::Property(inner, _) => {
                self.validate_order_by_expr_strict_with_inner(
                    inner,
                    with_names,
                    grouping_key_vars,
                    inside_aggregation,
                )?;
            }
            Expr::BinaryOp { left, right, .. } => {
                self.validate_order_by_expr_strict_with_inner(
                    left,
                    with_names,
                    grouping_key_vars,
                    inside_aggregation,
                )?;
                self.validate_order_by_expr_strict_with_inner(
                    right,
                    with_names,
                    grouping_key_vars,
                    inside_aggregation,
                )?;
            }
            Expr::UnaryOp { expr, .. } => {
                self.validate_order_by_expr_strict_with_inner(
                    expr,
                    with_names,
                    grouping_key_vars,
                    inside_aggregation,
                )?;
            }
            _ => {}
        }
        Ok(())
    }

    fn reset_scope_for_with(&mut self, wc: &WithClause) {
        let mut new_vars = HashMap::new();
        for item in &wc.items {
            if let Some(ref alias) = item.alias {
                // Preserve the kind if the expression is a direct variable reference.
                // For literals (known scalars), use Value.
                // For function calls and other complex expressions use Untyped so
                // a subsequent MATCH can treat the variable as a node if needed
                // (e.g. `WITH coalesce(a, b) AS x MATCH (x)-->(d)`).
                let kind = match &item.expr {
                    Expr::Variable(ref id) => {
                        self.vars.get(&id.0).copied().unwrap_or(VarKind::Value)
                    }
                    // Known non-node scalar literals.
                    Expr::Integer(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) => {
                        VarKind::Value
                    }
                    // Null and all other expressions (function calls, properties, etc.)
                    // are Untyped — their runtime type is not known at compile time.
                    // This allows e.g. `WITH null AS a OPTIONAL MATCH (a)-->()`.
                    _ => VarKind::Untyped,
                };
                new_vars.insert(alias.0.clone(), kind);
            } else if let Expr::Variable(ref id) = item.expr {
                let kind = self.vars.get(&id.0).copied().unwrap_or(VarKind::Value);
                new_vars.insert(id.0.clone(), kind);
            }
        }
        self.vars = new_vars;
        self.has_read = false;
        self.has_write = false;
    }

    // ── UNION validation ───────────────────────────────────────────────────

    fn validate_unions(&self, query: &Query) -> Result<(), GqlError> {
        // Check that all UNION parts are either all UNION or all UNION ALL (no mixing)
        if query.unions.len() >= 2 {
            let first_is_all = query.unions[0].0;
            for (i, (is_all, _)) in query.unions.iter().enumerate().skip(1) {
                if *is_all != first_is_all {
                    return Err(plan_error_syntax(
                        "invalid clause composition: cannot mix UNION and UNION ALL",
                    ));
                }
                let _ = i;
            }
        }

        // Check that all UNION parts have the same number and names of RETURN columns
        let first_return = Self::find_return_columns(&query.clauses);
        for (_, part_clauses) in &query.unions {
            let part_return = Self::find_return_columns(part_clauses);
            if let (Some(ref first), Some(ref part)) = (&first_return, &part_return) {
                if first.len() != part.len() {
                    return Err(plan_error_syntax(
                        "all sub queries in a UNION must have the same column names",
                    ));
                }
                for (f, p) in first.iter().zip(part.iter()) {
                    if f != p {
                        return Err(plan_error_syntax(
                            "all sub queries in a UNION must have the same column names",
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn find_return_columns(clauses: &[Clause]) -> Option<Vec<String>> {
        for clause in clauses {
            if let Clause::Return(rc) = clause {
                let names: Vec<String> = rc
                    .items
                    .iter()
                    .map(|item| {
                        if let Some(ref alias) = item.alias {
                            alias.0.clone()
                        } else if let Expr::Variable(ref id) = item.expr {
                            id.0.clone()
                        } else {
                            // Expression without alias
                            String::new()
                        }
                    })
                    .collect();
                return Some(names);
            }
        }
        None
    }

    // ── UNWIND validation ───────────────────────────────────────────────────

    fn validate_unwind(&mut self, uc: &UnwindClause) -> Result<(), GqlError> {
        self.validate_expr_general(&uc.expr)?;
        // UNWIND can produce any type (nodes, relationships, scalars),
        // so use Untyped to avoid false conflicts in subsequent MATCH patterns
        self.vars.insert(uc.alias.0.clone(), VarKind::Untyped);
        Ok(())
    }

    // ── SET / REMOVE validation ─────────────────────────────────────────────

    fn validate_set(&self, sc: &SetClause) -> Result<(), GqlError> {
        let empty_set = std::collections::HashSet::new();
        for item in &sc.items {
            match item {
                crate::ast::SetClauseItem::SetProperty { alias, value, .. } => {
                    if !self.vars.contains_key(&alias.0) {
                        return Err(plan_error_syntax(&format!(
                            "variable '{}' not defined",
                            alias.0
                        )));
                    }
                    // Check for PatternExpr in SET values (UnexpectedSyntax)
                    self.check_no_pattern_expr(value)?;
                    self.check_vars_defined(value, &empty_set)?;
                }
                crate::ast::SetClauseItem::SetMap { alias, map, .. } => {
                    if !self.vars.contains_key(&alias.0) {
                        return Err(plan_error_syntax(&format!(
                            "variable '{}' not defined",
                            alias.0
                        )));
                    }
                    self.check_no_pattern_expr(map)?;
                    self.check_vars_defined(map, &empty_set)?;
                }
                crate::ast::SetClauseItem::SetLabels { alias, .. } => {
                    if !self.vars.contains_key(&alias.0) {
                        return Err(plan_error_syntax(&format!(
                            "variable '{}' not defined",
                            alias.0
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_remove(&self, _rc: &RemoveClause) -> Result<(), GqlError> {
        Ok(())
    }

    // ── MERGE validation ────────────────────────────────────────────────────

    fn validate_merge(&mut self, mc: &MergeClause) -> Result<(), GqlError> {
        // Check if start node variable is already bound (VariableAlreadyBound for MERGE)
        if let Some(ref var) = mc.pattern.start.variable {
            if self.vars.contains_key(&var.0) {
                let has_labels = !mc.pattern.start.labels.is_empty();
                let has_props = mc
                    .pattern
                    .start
                    .properties
                    .as_ref()
                    .is_some_and(|p| !p.is_empty());
                if has_labels || has_props {
                    return Err(plan_error_syntax(&format!(
                        "variable '{}' already bound, cannot be rebound in MERGE with labels/properties",
                        var.0
                    )));
                }
                // Bare MERGE (a) after MATCH (a) is also VariableAlreadyBound
                // (when there are no steps — it's just re-creating)
                if mc.pattern.steps.is_empty() {
                    return Err(plan_error_syntax(&format!(
                        "variable '{}' already bound, cannot be used in MERGE",
                        var.0
                    )));
                }
            }
        }

        for step in &mc.pattern.steps {
            let rel = &step.rel;
            if rel.rel_types.is_empty() {
                return Err(plan_error_syntax(
                    "MERGE requires exactly one relationship type",
                ));
            }
            if rel.rel_types.len() > 1 {
                return Err(plan_error_syntax(
                    "MERGE requires exactly one relationship type, got multiple",
                ));
            }
            // Note: MERGE with undirected relationships is valid in some contexts
            if rel.range.is_some() {
                return Err(plan_error_syntax(
                    "variable-length relationship patterns are not allowed in MERGE",
                ));
            }
            // Check end nodes in MERGE steps for VariableAlreadyBound
            if let Some(ref var) = step.node.variable {
                if self.vars.contains_key(&var.0) {
                    let has_labels = !step.node.labels.is_empty();
                    let has_props = step.node.properties.is_some();
                    if has_labels || has_props {
                        return Err(plan_error_syntax(&format!(
                            "variable '{}' already bound, cannot be rebound in MERGE with labels/properties",
                            var.0
                        )));
                    }
                }
            }
        }
        // Register variables
        if let Some(ref var) = mc.pattern.variable {
            self.vars.insert(var.0.clone(), VarKind::Path);
        }
        if let Some(ref var) = mc.pattern.start.variable {
            self.vars.insert(var.0.clone(), VarKind::Node);
        }
        for step in &mc.pattern.steps {
            if let Some(ref var) = step.rel.variable {
                self.vars.insert(var.0.clone(), VarKind::Relationship);
            }
            if let Some(ref node_var) = step.node.variable {
                let name = node_var.0.clone();
                // If already bound and has new labels/properties, VariableAlreadyBound
                if self.vars.contains_key(&name) {
                    let has_labels = !step.node.labels.is_empty();
                    let has_props = step.node.properties.is_some();
                    if has_labels || has_props {
                        return Err(plan_error_syntax(&format!(
                            "variable '{}' already bound, cannot be rebound in MERGE with labels/properties",
                            name
                        )));
                    }
                }
                self.vars.insert(name, VarKind::Node);
            }
        }

        // Validate ON CREATE SET / ON MATCH SET — referenced variables must be in scope
        for item in &mc.on_create {
            self.validate_set_item_refs(item)?;
        }
        for item in &mc.on_match {
            self.validate_set_item_refs(item)?;
        }
        Ok(())
    }

    fn validate_set_item_refs(&self, item: &crate::ast::SetClauseItem) -> Result<(), GqlError> {
        match item {
            crate::ast::SetClauseItem::SetProperty { alias, .. } => {
                if !self.vars.contains_key(&alias.0) {
                    return Err(plan_error_syntax(&format!(
                        "variable '{}' not defined",
                        alias.0
                    )));
                }
            }
            crate::ast::SetClauseItem::SetMap { alias, .. } => {
                if !self.vars.contains_key(&alias.0) {
                    return Err(plan_error_syntax(&format!(
                        "variable '{}' not defined",
                        alias.0
                    )));
                }
            }
            crate::ast::SetClauseItem::SetLabels { alias, .. } => {
                if !self.vars.contains_key(&alias.0) {
                    return Err(plan_error_syntax(&format!(
                        "variable '{}' not defined",
                        alias.0
                    )));
                }
            }
        }
        Ok(())
    }

    // ── General expression validation ───────────────────────────────────────

    /// Validate expressions for compile-time type errors.
    /// This catches: `NOT <non-boolean-literal>`, `<non-boolean> AND/OR <expr>`.
    fn validate_expr_general(&self, expr: &Expr) -> Result<(), GqlError> {
        match expr {
            // Check for floating-point overflow (e.g. 1.34E999 → Infinity)
            Expr::Float(f) => {
                if f.is_infinite() {
                    return Err(plan_error_syntax(
                        "floating-point literal is too large (overflow to infinity)",
                    ));
                }
                return Ok(());
            }
            Expr::UnaryOp {
                op: UnaryOp::Not,
                expr: inner,
            } => {
                self.validate_expr_general(inner)?;
                // NOT on a non-boolean literal is a compile-time error
                if is_non_boolean_literal(inner) {
                    return Err(plan_error_syntax(
                        "NOT requires a boolean argument, got a non-boolean literal",
                    ));
                }
            }
            Expr::UnaryOp {
                op: UnaryOp::Neg,
                expr: inner,
            } => {
                self.validate_expr_general(inner)?;
                // -<non-numeric literal> is a type error
                if is_non_numeric_literal(inner) {
                    return Err(plan_error_syntax("unary minus requires a numeric argument"));
                }
            }
            Expr::BinaryOp { op, left, right } => {
                self.validate_expr_general(left)?;
                self.validate_expr_general(right)?;

                match op {
                    BinaryOp::And | BinaryOp::Or | BinaryOp::Xor => {
                        // Boolean operators on non-boolean literals
                        if is_non_boolean_literal(left) || is_non_boolean_literal(right) {
                            return Err(plan_error_syntax(&format!(
                                "{} requires boolean arguments, got a non-boolean literal",
                                op_name(*op)
                            )));
                        }
                    }
                    BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::Div
                    | BinaryOp::Mod
                    | BinaryOp::Pow => {
                        // Arithmetic on booleans is caught at runtime
                    }
                    BinaryOp::StartsWith
                    | BinaryOp::EndsWith
                    | BinaryOp::Contains
                    | BinaryOp::RegexMatch => {
                        // String operations on non-string literals
                        if is_non_string_non_null_literal(left)
                            || is_non_string_non_null_literal(right)
                        {
                            return Err(plan_error_syntax(&format!(
                                "{} requires string arguments",
                                op_name(*op)
                            )));
                        }
                    }
                    _ => {}
                }
            }
            Expr::FunctionCall { name, args, .. } => {
                for arg in args {
                    self.validate_expr_general(arg)?;
                }
                // Validate function is known
                validate_known_function(name.as_str())?;
                // Validate function argument counts for known functions at compile time
                validate_function_arity(name.as_str(), args)?;
                // Validate argument kinds for type-restricted functions
                self.validate_function_arg_kinds(name.as_str(), args)?;
                // Check for non-constant (non-deterministic) functions inside aggregation
                if is_aggregation_function(name.as_str()) {
                    self.check_no_nondeterministic_in_agg(name.as_str(), args)?;
                }
            }
            Expr::Property(inner, _) => {
                self.validate_expr_general(inner)?;
            }
            Expr::IsNull(inner) | Expr::IsNotNull(inner) => {
                self.validate_expr_general(inner)?;
            }
            Expr::Subscript(a, b) => {
                self.validate_expr_general(a)?;
                self.validate_expr_general(b)?;
            }
            Expr::Slice(a, b, c) => {
                self.validate_expr_general(a)?;
                if let Some(b) = b {
                    self.validate_expr_general(b)?;
                }
                if let Some(c) = c {
                    self.validate_expr_general(c)?;
                }
            }
            Expr::List(items) => {
                for item in items {
                    self.validate_expr_general(item)?;
                }
            }
            Expr::MapLiteral(pairs) => {
                for (_, v) in pairs {
                    self.validate_expr_general(v)?;
                }
            }
            Expr::Case {
                subject,
                alternatives,
                default,
            } => {
                if let Some(s) = subject {
                    self.validate_expr_general(s)?;
                }
                for (w, t) in alternatives {
                    self.validate_expr_general(w)?;
                    self.validate_expr_general(t)?;
                }
                if let Some(d) = default {
                    self.validate_expr_general(d)?;
                }
            }
            Expr::ListComp {
                list, filter, map, ..
            } => {
                self.validate_expr_general(list)?;
                if let Some(f) = filter {
                    self.validate_expr_general(f)?;
                }
                // Aggregation functions are not allowed inside list comprehension body
                if contains_aggregation(map) {
                    return Err(plan_error_syntax(
                        "aggregation functions are not allowed inside list comprehension",
                    ));
                }
                if let Some(f) = filter {
                    if contains_aggregation(f) {
                        return Err(plan_error_syntax(
                            "aggregation functions are not allowed inside list comprehension filter",
                        ));
                    }
                }
                self.validate_expr_general(map)?;
            }
            _ => {}
        }
        Ok(())
    }

    // ── ORDER BY validation ─────────────────────────────────────────────────

    fn validate_order_by_expr(
        &self,
        expr: &Expr,
        _return_items: &[ReturnItem],
    ) -> Result<(), GqlError> {
        // Validate type errors in ORDER BY expressions
        self.validate_expr_general(expr)
    }

    fn validate_order_by_expr_in_with(
        &self,
        expr: &Expr,
        with_names: &HashSet<String>,
    ) -> Result<(), GqlError> {
        // Variables in ORDER BY must be in the projected WITH columns or current scope
        self.check_expr_scope_in_with(expr, with_names)
    }

    fn check_expr_scope_in_with(
        &self,
        expr: &Expr,
        with_names: &HashSet<String>,
    ) -> Result<(), GqlError> {
        match expr {
            Expr::Variable(ref id) => {
                if !with_names.contains(&id.0) && !self.vars.contains_key(&id.0) {
                    return Err(plan_error_syntax(&format!(
                        "variable '{}' not defined",
                        id.0
                    )));
                }
            }
            Expr::Property(inner, _) => {
                self.check_expr_scope_in_with(inner, with_names)?;
            }
            Expr::BinaryOp { left, right, .. } => {
                self.check_expr_scope_in_with(left, with_names)?;
                self.check_expr_scope_in_with(right, with_names)?;
            }
            Expr::UnaryOp { expr, .. } => {
                self.check_expr_scope_in_with(expr, with_names)?;
            }
            Expr::FunctionCall { args, .. } => {
                for arg in args {
                    self.check_expr_scope_in_with(arg, with_names)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ── Column name conflict checking ───────────────────────────────────────

    fn check_column_name_conflicts(&self, items: &[ReturnItem]) -> Result<(), GqlError> {
        let mut names: HashSet<String> = HashSet::new();
        for item in items {
            // Only check explicit aliases and bare variable names
            let name = if let Some(ref alias) = item.alias {
                alias.0.clone()
            } else if let Expr::Variable(ref id) = item.expr {
                id.0.clone()
            } else {
                continue;
            };
            if !names.insert(name.clone()) {
                return Err(plan_error_syntax(&format!(
                    "column name '{name}' appears multiple times in RETURN"
                )));
            }
        }
        Ok(())
    }

    fn check_with_column_conflicts(&self, items: &[ReturnItem]) -> Result<(), GqlError> {
        let mut names: HashSet<String> = HashSet::new();
        for item in items {
            let name = if let Some(ref alias) = item.alias {
                alias.0.clone()
            } else if let Expr::Variable(ref id) = item.expr {
                id.0.clone()
            } else {
                continue;
            };
            if !names.insert(name.clone()) {
                return Err(plan_error_syntax(&format!(
                    "column name '{name}' appears multiple times in WITH"
                )));
            }
        }
        Ok(())
    }

    // ── Aggregation validation ──────────────────────────────────────────────

    fn validate_aggregation_consistency(&self, items: &[ReturnItem]) -> Result<(), GqlError> {
        let has_agg = items.iter().any(|item| contains_aggregation(&item.expr));
        if !has_agg {
            return Ok(());
        }
        for item in items {
            if has_nested_aggregation(&item.expr, false) {
                return Err(plan_error_syntax("nested aggregation is not allowed"));
            }
            // Check for ambiguous aggregation: expression that contains both
            // aggregation functions AND non-simple references at the same level.
            // e.g. `me.age + you.age + count(*)` where the addition mixes
            // aggregated and non-aggregated parts.
            if has_ambiguous_aggregation(&item.expr) {
                return Err(plan_error_syntax(
                    "ambiguous aggregation expression: contains both aggregation and non-aggregation",
                ));
            }
        }
        Ok(())
    }

    // ── Function argument type validation ──────────────────────────────────

    /// Validate that type-restricted functions receive the correct argument kinds.
    fn validate_function_arg_kinds(
        &self,
        name: &str,
        args: &[Expr],
    ) -> Result<(), GqlError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "type" => {
                // type(r) requires a Relationship argument
                if let Some(Expr::Variable(ref id)) = args.first() {
                    if let Some(&kind) = self.vars.get(&id.0) {
                        if kind == VarKind::Node {
                            return Err(plan_error_syntax(
                                "type() requires a relationship argument, got a node",
                            ));
                        }
                    }
                }
            }
            "length" => {
                // length(p) requires a Path argument — not a node or relationship
                if let Some(Expr::Variable(ref id)) = args.first() {
                    if let Some(&kind) = self.vars.get(&id.0) {
                        if kind == VarKind::Node || kind == VarKind::Relationship {
                            return Err(plan_error_syntax(
                                "length() requires a path argument, not a node or relationship",
                            ));
                        }
                    }
                }
            }
            "size" => {
                // size(p) where p is a path variable is invalid (InvalidArgumentType)
                if let Some(Expr::Variable(ref id)) = args.first() {
                    if let Some(&kind) = self.vars.get(&id.0) {
                        if kind == VarKind::Path {
                            return Err(plan_error_syntax(
                                "size() requires a list or string argument, not a path",
                            ));
                        }
                    }
                }
                // size(pattern) where argument is a PatternExpr is UnexpectedSyntax
                if let Some(arg) = args.first() {
                    if matches!(arg, Expr::PatternExpr(_)) {
                        return Err(plan_error_syntax(
                            "size() does not accept a pattern expression",
                        ));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Check that aggregation functions don't contain non-deterministic (non-constant) functions.
    fn check_no_nondeterministic_in_agg(
        &self,
        _agg_name: &str,
        args: &[Expr],
    ) -> Result<(), GqlError> {
        for arg in args {
            if self.contains_nondeterministic(arg) {
                return Err(plan_error_syntax(
                    "non-deterministic functions (like rand()) are not allowed inside aggregation",
                ));
            }
        }
        Ok(())
    }

    fn contains_nondeterministic(&self, expr: &Expr) -> bool {
        match expr {
            Expr::FunctionCall { name, args, .. } => {
                if is_nondeterministic_function(name.as_str()) {
                    return true;
                }
                args.iter().any(|a| self.contains_nondeterministic(a))
            }
            Expr::BinaryOp { left, right, .. } => {
                self.contains_nondeterministic(left) || self.contains_nondeterministic(right)
            }
            Expr::UnaryOp { expr, .. } => self.contains_nondeterministic(expr),
            Expr::Property(inner, _) => self.contains_nondeterministic(inner),
            Expr::List(items) => items.iter().any(|i| self.contains_nondeterministic(i)),
            _ => false,
        }
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
                if id.0 != "*"
                    && !self.vars.contains_key(&id.0)
                    && !extra_vars.contains(&id.0)
                {
                    return Err(plan_error_syntax(&format!(
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
            Expr::PatternExpr(_) => Err(plan_error_syntax(
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
            Expr::ListComp { list, filter, map, .. } => {
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
                    return Err(plan_error_syntax(&format!(
                        "{clause_name} requires a non-negative integer, got {n}"
                    )));
                }
            }
            Expr::Float(_) => {
                return Err(plan_error_syntax(&format!(
                    "{clause_name} requires an integer, got a floating point value"
                )));
            }
            Expr::UnaryOp {
                op: UnaryOp::Neg,
                expr: inner,
            } => match inner.as_ref() {
                Expr::Integer(_) => {
                    return Err(plan_error_syntax(&format!(
                        "{clause_name} requires a non-negative integer"
                    )));
                }
                Expr::Float(_) => {
                    return Err(plan_error_syntax(&format!(
                        "{clause_name} requires an integer, got a floating point value"
                    )));
                }
                _ => {}
            },
            Expr::Str(_) | Expr::Bool(_) | Expr::Null | Expr::List(_) | Expr::MapLiteral(_) => {
                return Err(plan_error_syntax(&format!(
                    "{clause_name} requires an integer expression"
                )));
            }
            _ => {}
        }
        Ok(())
    }
}

// ── Literal classification helpers ──────────────────────────────────────────

/// Returns true if expr contains no variable references (pure literal/arithmetic).
fn is_non_variable_expr(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Integer(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Null
    )
}

/// Returns true if expr is a non-boolean literal (includes all literal types except Bool and Null).
fn is_non_boolean_literal(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Integer(_) | Expr::Float(_) | Expr::Str(_) | Expr::List(_) | Expr::MapLiteral(_)
    )
}

fn is_non_numeric_literal(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Str(_) | Expr::Bool(_) | Expr::List(_) | Expr::MapLiteral(_)
    )
}

fn is_non_string_non_null_literal(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Integer(_) | Expr::Float(_) | Expr::Bool(_) | Expr::List(_) | Expr::MapLiteral(_)
    )
}

// ── Aggregation helpers ─────────────────────────────────────────────────────

fn contains_aggregation(expr: &Expr) -> bool {
    match expr {
        Expr::FunctionCall { name, args, .. } => {
            if is_aggregation_function(name.as_str()) {
                return true;
            }
            args.iter().any(contains_aggregation)
        }
        Expr::BinaryOp { left, right, .. } => {
            contains_aggregation(left) || contains_aggregation(right)
        }
        Expr::UnaryOp { expr, .. } => contains_aggregation(expr),
        Expr::Property(inner, _) => contains_aggregation(inner),
        Expr::IsNull(inner) | Expr::IsNotNull(inner) => contains_aggregation(inner),
        Expr::Subscript(a, b) => contains_aggregation(a) || contains_aggregation(b),
        Expr::Slice(a, b, c) => {
            contains_aggregation(a)
                || b.as_ref().is_some_and(|e| contains_aggregation(e))
                || c.as_ref().is_some_and(|e| contains_aggregation(e))
        }
        Expr::Case {
            subject,
            alternatives,
            default,
        } => {
            subject.as_ref().is_some_and(|s| contains_aggregation(s))
                || alternatives
                    .iter()
                    .any(|(w, t)| contains_aggregation(w) || contains_aggregation(t))
                || default.as_ref().is_some_and(|d| contains_aggregation(d))
        }
        Expr::List(items) => items.iter().any(contains_aggregation),
        Expr::MapLiteral(pairs) => pairs.iter().any(|(_, v)| contains_aggregation(v)),
        Expr::ListComp {
            list, filter, map, ..
        } => {
            contains_aggregation(list)
                || filter.as_ref().is_some_and(|f| contains_aggregation(f))
                || contains_aggregation(map)
        }
        _ => false,
    }
}

/// Returns true if an expression has ambiguous aggregation — e.g. `a.x + count(*)`
/// where an arithmetic operator at the top level mixes aggregated and non-aggregated children,
/// AND those non-aggregated children are themselves complex expressions (not simple variables
/// or property accesses, which would be valid grouping keys).
fn has_ambiguous_aggregation(expr: &Expr) -> bool {
    match expr {
        Expr::BinaryOp { left, right, op } => {
            // Only check arithmetic operators
            if !matches!(
                op,
                BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::Div
                    | BinaryOp::Mod
                    | BinaryOp::Pow
                    | BinaryOp::Concat
            ) {
                return false;
            }
            let left_has = contains_aggregation(left);
            let right_has = contains_aggregation(right);
            if left_has && !right_has && !is_simple_grouping_key(right) {
                return true;
            }
            if right_has && !left_has && !is_simple_grouping_key(left) {
                return true;
            }
            false
        }
        _ => false,
    }
}

/// Returns true if expr is a simple grouping key (variable, property, or literal).
fn is_simple_grouping_key(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Variable(_)
            | Expr::Property(_, _)
            | Expr::Integer(_)
            | Expr::Float(_)
            | Expr::Str(_)
            | Expr::Bool(_)
            | Expr::Null
            | Expr::Parameter(_)
    )
}

fn has_nested_aggregation(expr: &Expr, inside_agg: bool) -> bool {
    match expr {
        Expr::FunctionCall { name, args, .. } => {
            if is_aggregation_function(name.as_str()) {
                if inside_agg {
                    return true;
                }
                return args.iter().any(|a| has_nested_aggregation(a, true));
            }
            args.iter().any(|a| has_nested_aggregation(a, inside_agg))
        }
        Expr::BinaryOp { left, right, .. } => {
            has_nested_aggregation(left, inside_agg) || has_nested_aggregation(right, inside_agg)
        }
        Expr::UnaryOp { expr, .. } => has_nested_aggregation(expr, inside_agg),
        Expr::Property(inner, _) => has_nested_aggregation(inner, inside_agg),
        Expr::IsNull(inner) | Expr::IsNotNull(inner) => has_nested_aggregation(inner, inside_agg),
        _ => false,
    }
}

/// Collect all variable names referenced in an expression.
fn collect_variable_refs(expr: &Expr, vars: &mut HashSet<String>) {
    match expr {
        Expr::Variable(ref id) => {
            if id.0 != "*" {
                vars.insert(id.0.clone());
            }
        }
        Expr::Property(inner, _) => collect_variable_refs(inner, vars),
        Expr::BinaryOp { left, right, .. } => {
            collect_variable_refs(left, vars);
            collect_variable_refs(right, vars);
        }
        Expr::UnaryOp { expr, .. } => collect_variable_refs(expr, vars),
        Expr::FunctionCall { args, .. } => {
            for arg in args {
                collect_variable_refs(arg, vars);
            }
        }
        Expr::IsNull(inner) | Expr::IsNotNull(inner) | Expr::HasLabel(inner, _) => {
            collect_variable_refs(inner, vars);
        }
        Expr::List(items) => {
            for item in items {
                collect_variable_refs(item, vars);
            }
        }
        Expr::MapLiteral(pairs) => {
            for (_, v) in pairs {
                collect_variable_refs(v, vars);
            }
        }
        Expr::Subscript(a, b) => {
            collect_variable_refs(a, vars);
            collect_variable_refs(b, vars);
        }
        _ => {}
    }
}

fn is_nondeterministic_function(name: &str) -> bool {
    matches!(name.to_ascii_lowercase().as_str(), "rand")
}

fn is_aggregation_function(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "count"
            | "sum"
            | "avg"
            | "min"
            | "max"
            | "collect"
            | "stdev"
            | "stdevp"
            | "percentilecont"
            | "percentiledisc"
    )
}

// ── Function name validation ────────────────────────────────────────────────

fn validate_known_function(name: &str) -> Result<(), GqlError> {
    let lower = name.to_ascii_lowercase();
    let known = matches!(
        lower.as_str(),
        "abs"
            | "ceil"
            | "floor"
            | "round"
            | "sqrt"
            | "sign"
            | "tostring"
            | "tointeger"
            | "toint"
            | "tofloat"
            | "toboolean"
            | "size"
            | "length"
            | "toupper"
            | "tolower"
            | "trim"
            | "ltrim"
            | "rtrim"
            | "left"
            | "right"
            | "split"
            | "replace"
            | "substring"
            | "head"
            | "last"
            | "tail"
            | "reverse"
            | "coalesce"
            | "isnull"
            | "range"
            | "type"
            | "keys"
            | "labels"
            | "nodes"
            | "relationships"
            | "id"
            | "properties"
            | "startnode"
            | "endnode"
            | "exists"
            | "rand"
            | "timestamp"
            | "pi"
            | "e"
            | "log"
            | "log10"
            | "exp"
            | "sin"
            | "cos"
            | "tan"
            | "asin"
            | "acos"
            | "atan"
            | "atan2"
            | "degrees"
            | "radians"
            | "haversin"
            | "count"
            | "sum"
            | "avg"
            | "min"
            | "max"
            | "collect"
            | "stdev"
            | "stdevp"
            | "percentilecont"
            | "percentiledisc"
            | "single"
            | "any"
            | "all"
            | "none"
            | "reduce"
            | "extract"
            | "filter"
            | "tobooleanlist"
            | "tointegerlist"
            | "tofloatlist"
            | "tostringlist"
            | "tobooleanornull"
            | "tointegerornull"
            | "tofloatornull"
            | "tostringornull"
            | "char_length"
            | "character_length"
            // Temporal functions
            | "date"
            | "datetime"
            | "localdatetime"
            | "time"
            | "localtime"
            | "duration"
            // Temporal namespaced functions
            | "date.truncate"
            | "datetime.truncate"
            | "localdatetime.truncate"
            | "localtime.truncate"
            | "time.truncate"
            | "date.realtime"
            | "date.statement"
            | "date.transaction"
            | "datetime.realtime"
            | "datetime.statement"
            | "datetime.transaction"
            | "datetime.fromepoch"
            | "datetime.fromepochmillis"
            | "localdatetime.realtime"
            | "localdatetime.statement"
            | "localdatetime.transaction"
            | "localtime.realtime"
            | "localtime.statement"
            | "localtime.transaction"
            | "time.realtime"
            | "time.statement"
            | "time.transaction"
            | "duration.between"
            | "duration.inmonths"
            | "duration.indays"
            | "duration.inseconds"
            // Spatial functions
            | "point"
            | "distance"
            // Internal pattern comprehension
            | "__patterncomp"
    );
    if !known {
        return Err(plan_error_syntax(&format!("unknown function '{name}'")));
    }
    Ok(())
}

// ── Function arity validation ───────────────────────────────────────────────

fn validate_function_arity(name: &str, args: &[Expr]) -> Result<(), GqlError> {
    let n = args.len();
    let lower_name = name.to_ascii_lowercase();
    let expected = match lower_name.as_str() {
        // Functions that take exactly 0 arguments
        "rand" | "timestamp" | "pi" | "e" => Some((0, 0)),
        // Functions that take exactly 1 argument
        "abs" | "ceil" | "floor" | "round" | "sqrt" | "sign" | "tostring" | "tointeger"
        | "toint" | "tofloat" | "toboolean" | "size" | "length" | "toupper" | "tolower"
        | "trim" | "ltrim" | "rtrim" | "head" | "last" | "tail" | "reverse" | "isnull" | "type"
        | "keys" | "labels" | "nodes" | "relationships" | "id" | "properties" | "startnode"
        | "endnode" | "exists" | "log" | "log10" | "exp" | "sin" | "cos" | "tan" | "asin"
        | "acos" | "atan" | "degrees" | "radians" | "haversin" => Some((1, 1)),
        // Functions that take exactly 2 arguments
        "left" | "right" | "split" | "atan2" => Some((2, 2)),
        // Functions that take exactly 3 arguments
        "replace" => Some((3, 3)),
        // Functions that take 2-3 arguments
        "substring" => Some((2, 3)),
        // Functions that take variable arguments
        "range" => Some((2, 3)),
        "coalesce" => {
            if n == 0 {
                return Err(plan_error_syntax("coalesce() requires at least 1 argument"));
            }
            None
        }
        // Temporal functions: 0 or 1 argument
        "date" | "datetime" | "localdatetime" | "time" | "localtime" | "duration" => Some((0, 1)),
        // Spatial functions
        "point" => Some((1, 1)),
        "distance" => Some((2, 2)),
        // Aggregation functions: 1 arg (or 0 for count(*))
        "count" => Some((0, 1)),
        "sum" | "avg" | "collect" | "stdev" | "stdevp" | "min" | "max" => Some((1, 1)),
        "percentilecont" | "percentiledisc" => Some((2, 2)),
        _ => None, // Unknown functions are handled elsewhere
    };
    if let Some((min, max)) = expected {
        if n < min || n > max {
            return Err(plan_error_syntax(&format!(
                "function {name}() expects {}{} argument(s), got {n}",
                min,
                if min != max {
                    format!("-{max}")
                } else {
                    String::new()
                }
            )));
        }
    }
    Ok(())
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

fn plan_error_syntax(msg: &str) -> GqlError {
    GqlError::Plan(PlanError::UndefinedVariable {
        name: msg.to_string(),
        offset: 0,
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
