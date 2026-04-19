//! `shortestPath(...)` and `allShortestPaths(...)` pattern planning.

use crate::ast::{Identifier, PathPattern, Properties, ShortestPathMode};
use crate::error::{GqlError, PlanError};
use crate::planner::Operator;

use super::super::helpers::{label_filter_expr, param_props_filter_expr, props_to_filter_expr};
use super::super::QueryPlanner;

impl<'s> QueryPlanner<'s> {
    /// Plan a `shortestPath(...)` or `allShortestPaths(...)` pattern.
    ///
    /// The pattern must have exactly one step with a variable-length relationship.
    /// Generates:
    /// ```text
    /// Apply {
    ///   outer: NodeScan(start) + filters,
    ///   inner: Apply {
    ///     outer: NodeScan(end) + filters,
    ///     inner: ShortestPath { src, dst, ... }
    ///   }
    /// }
    /// ```
    pub(super) fn plan_shortest_path_pattern(
        &self,
        pattern: &PathPattern,
        mode: ShortestPathMode,
    ) -> Result<Operator, GqlError> {
        if pattern.steps.len() != 1 {
            return Err(GqlError::Plan(PlanError::SemanticError {
                message: "shortestPath requires exactly one relationship step".to_string(),
            }));
        }
        let step = &pattern.steps[0];
        let range = step.rel.range.as_ref().ok_or_else(|| {
            GqlError::Plan(PlanError::SemanticError {
                message: "shortestPath requires a variable-length relationship (use [*] or [*..N])"
                    .to_string(),
            })
        })?;

        let (min, max) = crate::planner::validate_varlen_bounds(range.min.unwrap_or(1), range.max)?;

        let start = &pattern.start;
        let src_alias = start
            .variable
            .clone()
            .unwrap_or_else(|| Identifier::new("__sp_src__"));
        let start_label = start.labels.first().cloned();

        let end = &step.node;
        let dst_alias = end
            .variable
            .clone()
            .unwrap_or_else(|| Identifier::new("__sp_dst__"));
        let end_label = end.labels.first().cloned();

        let path_alias = pattern
            .variable
            .clone()
            .unwrap_or_else(|| Identifier::new("__sp_path__"));

        // Build start node scan + filters.
        let mut src_op = self.plan_node_scan(src_alias.clone(), start_label);
        if let Some(Properties::Map(props)) = &start.properties {
            if !props.is_empty() {
                let pred = props_to_filter_expr(&src_alias, props);
                src_op = Operator::Filter {
                    predicate: pred,
                    child: Box::new(src_op),
                };
            }
        } else if let Some(Properties::Parameter(name)) = &start.properties {
            let pred = param_props_filter_expr(&src_alias, name);
            src_op = Operator::Filter {
                predicate: pred,
                child: Box::new(src_op),
            };
        }
        if start.labels.len() > 1 {
            for extra_label in &start.labels[1..] {
                let pred = label_filter_expr(&src_alias, extra_label);
                src_op = Operator::Filter {
                    predicate: pred,
                    child: Box::new(src_op),
                };
            }
        }
        if let Some(pred) = &start.where_clause {
            src_op = Operator::Filter {
                predicate: pred.clone(),
                child: Box::new(src_op),
            };
        }

        // Build end node scan + filters.
        let mut dst_op = self.plan_node_scan(dst_alias.clone(), end_label);
        if let Some(Properties::Map(props)) = &end.properties {
            if !props.is_empty() {
                let pred = props_to_filter_expr(&dst_alias, props);
                dst_op = Operator::Filter {
                    predicate: pred,
                    child: Box::new(dst_op),
                };
            }
        } else if let Some(Properties::Parameter(name)) = &end.properties {
            let pred = param_props_filter_expr(&dst_alias, name);
            dst_op = Operator::Filter {
                predicate: pred,
                child: Box::new(dst_op),
            };
        }
        if end.labels.len() > 1 {
            for extra_label in &end.labels[1..] {
                let pred = label_filter_expr(&dst_alias, extra_label);
                dst_op = Operator::Filter {
                    predicate: pred,
                    child: Box::new(dst_op),
                };
            }
        }
        if let Some(pred) = &end.where_clause {
            dst_op = Operator::Filter {
                predicate: pred.clone(),
                child: Box::new(dst_op),
            };
        }

        // Build ShortestPath operator.
        let sp_op = Operator::ShortestPath {
            src_alias,
            dst_alias,
            rel_types: step.rel.rel_types.clone(),
            direction: step.rel.direction,
            min,
            max,
            path_alias,
            all_paths: mode == ShortestPathMode::All,
        };

        // Apply(outer=src_scan, inner=Apply(outer=dst_scan, inner=shortest_path))
        let inner_apply = Operator::Apply {
            outer: Box::new(dst_op),
            inner: Box::new(sp_op),
        };
        Ok(Operator::Apply {
            outer: Box::new(src_op),
            inner: Box::new(inner_apply),
        })
    }
}
