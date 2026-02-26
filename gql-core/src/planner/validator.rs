//! Compile-time query validator for openCypher / GQL.
//!
//! [`validate_query`] runs after parsing but before planning to catch semantic
//! errors that the grammar allows but the spec forbids. Each check maps to one
//! or more openCypher TCK error codes (e.g. `VariableAlreadyBound`,
//! `UndefinedVariable`, `InvalidClauseComposition`).

use std::collections::{HashMap, HashSet};

use crate::ast::{
    BinaryOp, Clause, CreateClause, DeleteClause, Direction, Expr, MatchClause, MergeClause,
    NodePattern, PathPattern, Query, RelPattern, RemoveClause, ReturnClause, ReturnItem, SetClause,
    UnaryOp, UnwindClause, WithClause,
};
use crate::error::{GqlError, ParseError, PlanError};

/// The kind of a variable in scope — used to detect type conflicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VarKind {
    Node,
    Relationship,
    Path,
    Value, // scalar from WITH / UNWIND / RETURN literal
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
                Clause::Call(_) => {
                    // CALL clauses are validated at the planner level.
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
                // Note: Value -> Node (e.g. WITH n AS a MATCH (a)) is allowed when
                // the value could be a node reference. We check at runtime.
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
        self.validate_expr_general(expr)
    }

    // ── CREATE validation ───────────────────────────────────────────────────

    fn validate_create(&mut self, cc: &CreateClause) -> Result<(), GqlError> {
        for pattern in &cc.patterns {
            self.validate_create_pattern(pattern)?;
        }
        Ok(())
    }

    fn validate_create_pattern(&mut self, pattern: &PathPattern) -> Result<(), GqlError> {
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
                let has_props = node.properties.as_ref().is_some_and(|p| !p.is_empty());
                // VariableAlreadyBound: re-use with labels or properties
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
        // Check RETURN * with empty scope
        if rc.items.is_empty() && self.vars.is_empty() {
            return Err(plan_error_syntax(
                "RETURN * requires at least one variable in scope",
            ));
        }
        self.check_column_name_conflicts(&rc.items)?;
        self.validate_aggregation_consistency(&rc.items)?;
        // Validate RETURN expressions for type errors
        for item in &rc.items {
            self.validate_expr_general(&item.expr)?;
        }
        // Validate ORDER BY — check for aggregation in ORDER BY
        let has_agg_in_return = rc.items.iter().any(|item| contains_aggregation(&item.expr));
        for sort_item in &rc.order_by {
            if !has_agg_in_return && contains_aggregation(&sort_item.expr) {
                return Err(plan_error_syntax(
                    "aggregation in ORDER BY requires aggregation in RETURN",
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

    // ── WITH validation ─────────────────────────────────────────────────────

    fn validate_with(&self, wc: &WithClause) -> Result<(), GqlError> {
        self.check_with_column_conflicts(&wc.items)?;
        self.validate_aggregation_consistency(&wc.items)?;
        for item in &wc.items {
            self.validate_expr_general(&item.expr)?;
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
        for sort_item in &wc.order_by {
            if !has_agg_in_projection && contains_aggregation(&sort_item.expr) {
                return Err(plan_error_syntax(
                    "aggregation in ORDER BY requires aggregation in WITH projection",
                ));
            }
            self.validate_order_by_expr_in_with(&sort_item.expr, &with_names)?;
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

    fn reset_scope_for_with(&mut self, wc: &WithClause) {
        let mut new_vars = HashMap::new();
        for item in &wc.items {
            if let Some(ref alias) = item.alias {
                new_vars.insert(alias.0.clone(), VarKind::Value);
            } else if let Expr::Variable(ref id) = item.expr {
                let kind = self.vars.get(&id.0).copied().unwrap_or(VarKind::Value);
                new_vars.insert(id.0.clone(), kind);
            }
        }
        self.vars = new_vars;
        self.has_read = false;
        self.has_write = false;
    }

    // ── UNWIND validation ───────────────────────────────────────────────────

    fn validate_unwind(&mut self, uc: &UnwindClause) -> Result<(), GqlError> {
        self.validate_expr_general(&uc.expr)?;
        self.vars.insert(uc.alias.0.clone(), VarKind::Value);
        Ok(())
    }

    // ── SET / REMOVE validation ─────────────────────────────────────────────

    fn validate_set(&self, _sc: &SetClause) -> Result<(), GqlError> {
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
        }
        // Register variables
        if let Some(ref var) = mc.pattern.start.variable {
            self.vars.insert(var.0.clone(), VarKind::Node);
        }
        for step in &mc.pattern.steps {
            if let Some(ref var) = step.rel.variable {
                self.vars.insert(var.0.clone(), VarKind::Relationship);
            }
            if let Some(ref var) = step.node.variable {
                self.vars.insert(var.0.clone(), VarKind::Node);
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
        }
        Ok(())
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
