//! Variable scope and binding validation for MATCH, CREATE, DELETE, UNWIND,
//! SET, REMOVE, and MERGE clauses.

use std::collections::HashSet;

use crate::ast::{
    CreateClause, DeleteClause, Direction, Expr, MatchClause, MergeClause, NodePattern,
    PathPattern, Properties, RelPattern, RemoveClause, SetClause, UnwindClause,
};
use crate::error::GqlError;

use super::expression::{contains_aggregation, is_non_variable_expr};
use super::{kind_name, plan_error_semantic, ValidationContext, VarKind};

impl ValidationContext {
    // ── MATCH validation ────────────────────────────────────────────────────

    pub(super) fn validate_match(&mut self, mc: &MatchClause) -> Result<(), GqlError> {
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
                    return Err(plan_error_semantic(&format!(
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
                    return Err(plan_error_semantic(&format!(
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
                    return Err(plan_error_semantic(&format!(
                        "variable '{name}' already defined as {}, cannot be used as Node",
                        kind_name(existing)
                    )));
                }
                // Value -> Node: e.g. `WITH true AS n MATCH (n)` is a type conflict
                // because a literal value cannot be used as a node pattern.
                // Untyped (from CALL YIELD or UNWIND) can become a Node — no conflict.
                if existing == VarKind::Value {
                    return Err(plan_error_semantic(&format!(
                        "variable '{name}' already defined as Value, cannot be used as Node",
                    )));
                }
            }
            self.vars.insert(name, VarKind::Node);
        }
        match &node.properties {
            Some(Properties::Map(props)) => {
                for (_, expr) in props {
                    self.validate_expr_general(expr)?;
                }
            }
            Some(Properties::Parameter(name)) => {
                return Err(plan_error_semantic(&format!(
                    "parameter map ${name} cannot be used as a property predicate in MATCH; \
                     use inline properties or a WHERE clause instead"
                )));
            }
            None => {}
        }
        Ok(())
    }

    fn validate_rel_pattern_in_match(&mut self, rel: &RelPattern) -> Result<(), GqlError> {
        if let Some(ref var) = rel.variable {
            let name = var.0.clone();
            if let Some(&existing) = self.vars.get(&name) {
                if existing == VarKind::Node || existing == VarKind::Path {
                    return Err(plan_error_semantic(&format!(
                        "variable '{name}' already defined as {}, cannot be used as Relationship",
                        kind_name(existing)
                    )));
                }
                // Value -> Relationship: e.g. `WITH true AS r MATCH ()-[r]-()` is a type conflict
                // Exception: variable-length rel patterns like `-[rs*]->` can use a list variable
                // Untyped (from CALL YIELD) can become a Relationship — no conflict.
                if existing == VarKind::Value && rel.range.is_none() {
                    return Err(plan_error_semantic(&format!(
                        "variable '{name}' already defined as Value, cannot be used as Relationship",
                    )));
                }
            }
            self.vars.insert(name, VarKind::Relationship);
        }
        match &rel.properties {
            Some(Properties::Map(props)) => {
                for (_, expr) in props {
                    self.validate_expr_general(expr)?;
                }
            }
            Some(Properties::Parameter(name)) => {
                return Err(plan_error_semantic(&format!(
                    "parameter map ${name} cannot be used as a property predicate in MATCH; \
                     use inline properties or a WHERE clause instead"
                )));
            }
            None => {}
        }
        Ok(())
    }

    // ── WHERE expression validation ─────────────────────────────────────────

    pub(super) fn validate_where_expr(&self, expr: &Expr) -> Result<(), GqlError> {
        // Aggregation functions are not allowed in WHERE clauses (InvalidAggregation)
        if contains_aggregation(expr) {
            return Err(plan_error_semantic(
                "aggregation functions are not allowed in WHERE clauses",
            ));
        }
        // A standalone node/relationship variable used as a boolean predicate is
        // InvalidArgumentType — e.g. `WHERE n` where n is a node
        if let Expr::Variable(ref id) = expr {
            if let Some(&kind) = self.vars.get(&id.0) {
                if kind == VarKind::Node || kind == VarKind::Relationship {
                    return Err(plan_error_semantic(&format!(
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
    pub(super) fn check_vars_defined_in_where(&self, expr: &Expr) -> Result<(), GqlError> {
        match expr {
            // PatternExpr in WHERE — variable scope is checked by check_where_pattern_vars
            Expr::PatternExpr(_) | Expr::ExistsSubquery { .. } => Ok(()),
            Expr::Variable(ref id) => {
                if id.0 != "*" && !self.vars.contains_key(&id.0) {
                    return Err(plan_error_semantic(&format!(
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
                            return Err(plan_error_semantic(&format!(
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
            Expr::Reduce {
                accumulator,
                initial,
                variable,
                list,
                expression,
            } => {
                self.check_vars_defined_in_where(initial)?;
                self.check_vars_defined_in_where(list)?;
                // Both accumulator and variable are in scope for the expression
                let mut inner_vars = self.vars.clone();
                inner_vars.insert(accumulator.0.clone(), VarKind::Value);
                inner_vars.insert(variable.0.clone(), VarKind::Value);
                let inner_ctx = ValidationContext {
                    vars: inner_vars,
                    has_read: self.has_read,
                    has_write: self.has_write,
                    return_count: self.return_count,
                };
                inner_ctx.check_vars_defined_in_where(expression)
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
                                return Err(plan_error_semantic(
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
                return Err(plan_error_semantic(&format!(
                    "variable '{}' not defined",
                    var.0
                )));
            }
        }
        for step in &pattern.steps {
            if let Some(ref var) = step.rel.variable {
                if !var.0.is_empty() && !self.vars.contains_key(&var.0) {
                    return Err(plan_error_semantic(&format!(
                        "variable '{}' not defined",
                        var.0
                    )));
                }
            }
            if let Some(ref var) = step.node.variable {
                if !var.0.is_empty() && !self.vars.contains_key(&var.0) {
                    return Err(plan_error_semantic(&format!(
                        "variable '{}' not defined",
                        var.0
                    )));
                }
            }
        }
        Ok(())
    }

    // ── CREATE validation ───────────────────────────────────────────────────

    pub(super) fn validate_create(&mut self, cc: &CreateClause) -> Result<(), GqlError> {
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
                    return Err(plan_error_semantic(&format!(
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
        // Register the path variable (e.g. `p` in `CREATE p = (a)-[:R]->(b)`)
        // so it is available in subsequent RETURN / WITH clauses.
        if let Some(ref path_var) = pattern.variable {
            let name = path_var.0.clone();
            if let Some(&existing_kind) = self.vars.get(&name) {
                if existing_kind != VarKind::Path {
                    return Err(plan_error_semantic(&format!(
                        "variable '{name}' already defined as {}, cannot rebind as Path",
                        kind_name(existing_kind)
                    )));
                }
            }
            self.vars.insert(name, VarKind::Path);
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
                    return Err(plan_error_semantic(&format!(
                        "variable '{name}' already bound, cannot be rebound in CREATE with labels/properties"
                    )));
                }
                // Note: bare reference like (a) in `CREATE (a)-[:KNOWS]->(b)` is OK
                // but standalone `CREATE (a)` is caught separately
            }
            self.vars.insert(name, VarKind::Node);
        }
        match &node.properties {
            Some(Properties::Map(props)) => {
                for (_, expr) in props {
                    self.validate_expr_in_create(expr)?;
                }
            }
            Some(Properties::Parameter(_)) => {
                // Parameter references are validated at runtime.
            }
            None => {}
        }
        Ok(())
    }

    fn validate_rel_in_create(&mut self, rel: &RelPattern) -> Result<(), GqlError> {
        if rel.rel_types.is_empty() {
            return Err(plan_error_semantic(
                "CREATE requires exactly one relationship type",
            ));
        }
        if rel.rel_types.len() > 1 {
            return Err(plan_error_semantic(
                "CREATE requires exactly one relationship type, got multiple",
            ));
        }
        if rel.direction == Direction::Undirected {
            return Err(plan_error_semantic(
                "CREATE requires a directed relationship pattern",
            ));
        }
        if rel.range.is_some() {
            return Err(plan_error_semantic(
                "variable-length relationship patterns are not allowed in CREATE",
            ));
        }
        if let Some(ref var) = rel.variable {
            let name = var.0.clone();
            if self.vars.contains_key(&name) {
                return Err(plan_error_semantic(&format!(
                    "variable '{name}' already bound, cannot be rebound in CREATE"
                )));
            }
            self.vars.insert(name, VarKind::Relationship);
        }
        match &rel.properties {
            Some(Properties::Map(props)) => {
                for (_, expr) in props {
                    self.validate_expr_in_create(expr)?;
                }
            }
            Some(Properties::Parameter(_)) => {
                // Parameter references are validated at runtime.
            }
            None => {}
        }
        Ok(())
    }

    fn validate_expr_in_create(&self, expr: &Expr) -> Result<(), GqlError> {
        match expr {
            Expr::Variable(ref id) => {
                if !self.vars.contains_key(&id.0) {
                    return Err(plan_error_semantic(&format!(
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

    pub(super) fn validate_delete(&self, dc: &DeleteClause) -> Result<(), GqlError> {
        for expr in &dc.exprs {
            self.validate_delete_expr(expr)?;
        }
        Ok(())
    }

    fn validate_delete_expr(&self, expr: &Expr) -> Result<(), GqlError> {
        match expr {
            Expr::Variable(ref id) => {
                if !self.vars.contains_key(&id.0) {
                    return Err(plan_error_semantic(&format!(
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
            Expr::HasLabel(_, _) => Err(plan_error_semantic(
                "invalid use of a label predicate in DELETE; use REMOVE to remove labels",
            )),
            // Literal values in DELETE are invalid: DELETE 1+1
            Expr::Integer(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Null => Err(
                plan_error_semantic("DELETE requires a variable reference, got a literal"),
            ),
            // Arithmetic expressions on literals: DELETE 1+1
            Expr::BinaryOp { left, right, .. } => {
                if is_non_variable_expr(left) && is_non_variable_expr(right) {
                    return Err(plan_error_semantic(
                        "DELETE requires a variable reference, got an expression",
                    ));
                }
                Ok(())
            }
            // Other expressions (function calls, etc.) — allow through for runtime checking
            _ => Ok(()),
        }
    }

    // ── UNWIND validation ───────────────────────────────────────────────────

    pub(super) fn validate_unwind(&mut self, uc: &UnwindClause) -> Result<(), GqlError> {
        self.validate_expr_general(&uc.expr)?;
        // UNWIND can produce any type (nodes, relationships, scalars),
        // so use Untyped to avoid false conflicts in subsequent MATCH patterns
        self.vars.insert(uc.alias.0.clone(), VarKind::Untyped);
        Ok(())
    }

    // ── SET / REMOVE validation ─────────────────────────────────────────────

    pub(super) fn validate_set(&self, sc: &SetClause) -> Result<(), GqlError> {
        let empty_set = std::collections::HashSet::new();
        for item in &sc.items {
            match item {
                crate::ast::SetClauseItem::SetProperty { alias, value, .. } => {
                    if !self.vars.contains_key(&alias.0) {
                        return Err(plan_error_semantic(&format!(
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
                        return Err(plan_error_semantic(&format!(
                            "variable '{}' not defined",
                            alias.0
                        )));
                    }
                    self.check_no_pattern_expr(map)?;
                    self.check_vars_defined(map, &empty_set)?;
                }
                crate::ast::SetClauseItem::SetLabels { alias, .. } => {
                    if !self.vars.contains_key(&alias.0) {
                        return Err(plan_error_semantic(&format!(
                            "variable '{}' not defined",
                            alias.0
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn validate_remove(&self, _rc: &RemoveClause) -> Result<(), GqlError> {
        Ok(())
    }

    // ── MERGE validation ────────────────────────────────────────────────────

    pub(super) fn validate_merge(&mut self, mc: &MergeClause) -> Result<(), GqlError> {
        // Check if start node variable is already bound (VariableAlreadyBound for MERGE)
        if let Some(ref var) = mc.pattern.start.variable {
            if self.vars.contains_key(&var.0) {
                let has_labels = !mc.pattern.start.labels.is_empty();
                let has_props = mc.pattern.start.properties.is_some();
                if has_labels || has_props {
                    return Err(plan_error_semantic(&format!(
                        "variable '{}' already bound, cannot be rebound in MERGE with labels/properties",
                        var.0
                    )));
                }
                // Bare MERGE (a) after MATCH (a) is also VariableAlreadyBound
                // (when there are no steps — it's just re-creating)
                if mc.pattern.steps.is_empty() {
                    return Err(plan_error_semantic(&format!(
                        "variable '{}' already bound, cannot be used in MERGE",
                        var.0
                    )));
                }
            }
        }

        for step in &mc.pattern.steps {
            let rel = &step.rel;
            if rel.rel_types.is_empty() {
                return Err(plan_error_semantic(
                    "MERGE requires exactly one relationship type",
                ));
            }
            if rel.rel_types.len() > 1 {
                return Err(plan_error_semantic(
                    "MERGE requires exactly one relationship type, got multiple",
                ));
            }
            // Note: MERGE with undirected relationships is valid in some contexts
            if rel.range.is_some() {
                return Err(plan_error_semantic(
                    "variable-length relationship patterns are not allowed in MERGE",
                ));
            }
            // Check end nodes in MERGE steps for VariableAlreadyBound
            if let Some(ref var) = step.node.variable {
                if self.vars.contains_key(&var.0) {
                    let has_labels = !step.node.labels.is_empty();
                    let has_props = step.node.properties.is_some();
                    if has_labels || has_props {
                        return Err(plan_error_semantic(&format!(
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
                        return Err(plan_error_semantic(&format!(
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
                    return Err(plan_error_semantic(&format!(
                        "variable '{}' not defined",
                        alias.0
                    )));
                }
            }
            crate::ast::SetClauseItem::SetMap { alias, .. } => {
                if !self.vars.contains_key(&alias.0) {
                    return Err(plan_error_semantic(&format!(
                        "variable '{}' not defined",
                        alias.0
                    )));
                }
            }
            crate::ast::SetClauseItem::SetLabels { alias, .. } => {
                if !self.vars.contains_key(&alias.0) {
                    return Err(plan_error_semantic(&format!(
                        "variable '{}' not defined",
                        alias.0
                    )));
                }
            }
        }
        Ok(())
    }
}
