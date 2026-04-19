//! MATCH clause validation: pattern variable binding and type tracking.

use std::collections::HashSet;

use crate::ast::{MatchClause, NodePattern, PathPattern, Properties, RelPattern};
use crate::error::GqlError;

use super::super::{kind_name, plan_error_semantic, ValidationContext, VarKind};

impl ValidationContext {
    pub(in crate::planner::validator) fn validate_match(
        &mut self,
        mc: &MatchClause,
    ) -> Result<(), GqlError> {
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
}
