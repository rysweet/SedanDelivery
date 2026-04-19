//! CREATE clause validation: pattern binding restrictions and expression checks.

use crate::ast::{CreateClause, Direction, Expr, NodePattern, PathPattern, Properties, RelPattern};
use crate::error::GqlError;

use super::super::{kind_name, plan_error_semantic, ValidationContext, VarKind};

impl ValidationContext {
    pub(in crate::planner::validator) fn validate_create(
        &mut self,
        cc: &CreateClause,
    ) -> Result<(), GqlError> {
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
}
