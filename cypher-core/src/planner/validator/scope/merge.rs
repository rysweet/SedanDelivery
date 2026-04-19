//! MERGE clause validation: pattern binding rules and ON CREATE / ON MATCH SET.

use crate::ast::MergeClause;
use crate::error::GqlError;

use super::super::{plan_error_semantic, ValidationContext, VarKind};

impl ValidationContext {
    pub(in crate::planner::validator) fn validate_merge(
        &mut self,
        mc: &MergeClause,
    ) -> Result<(), GqlError> {
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
