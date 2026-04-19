//! Projection-list validation: column-name conflicts and aggregation
//! consistency for both RETURN and WITH item lists.

use std::collections::HashSet;

use crate::ast::{Expr, ReturnItem};
use crate::error::GqlError;
use crate::planner::validator::expression::{
    contains_aggregation, has_ambiguous_aggregation, has_nested_aggregation,
};
use crate::planner::validator::{plan_error_semantic, ValidationContext};

impl ValidationContext {
    pub(in crate::planner::validator) fn check_column_name_conflicts(
        &self,
        items: &[ReturnItem],
    ) -> Result<(), GqlError> {
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
                return Err(plan_error_semantic(&format!(
                    "column name '{name}' appears multiple times in RETURN"
                )));
            }
        }
        Ok(())
    }

    pub(in crate::planner::validator) fn check_with_column_conflicts(
        &self,
        items: &[ReturnItem],
    ) -> Result<(), GqlError> {
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
                return Err(plan_error_semantic(&format!(
                    "column name '{name}' appears multiple times in WITH"
                )));
            }
        }
        Ok(())
    }

    pub(in crate::planner::validator) fn validate_aggregation_consistency(
        &self,
        items: &[ReturnItem],
    ) -> Result<(), GqlError> {
        let has_agg = items.iter().any(|item| contains_aggregation(&item.expr));
        if !has_agg {
            return Ok(());
        }
        for item in items {
            if has_nested_aggregation(&item.expr, false) {
                return Err(plan_error_semantic("nested aggregation is not allowed"));
            }
            // Check for ambiguous aggregation: expression that contains both
            // aggregation functions AND non-simple references at the same level.
            // e.g. `me.age + you.age + count(*)` where the addition mixes
            // aggregated and non-aggregated parts.
            if has_ambiguous_aggregation(&item.expr) {
                return Err(plan_error_semantic(
                    "ambiguous aggregation expression: contains both aggregation and non-aggregation",
                ));
            }
        }
        Ok(())
    }
}
