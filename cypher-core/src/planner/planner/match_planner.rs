//! MATCH and OPTIONAL MATCH planning — pattern compilation and edge expansion.

use crate::ast::{
    Expr, Identifier, Label, MatchClause, PathPattern, PathStep, Properties, ShortestPathMode,
};
use crate::error::{GqlError, PlanError};
use crate::planner::Operator;

use super::helpers::{label_filter_expr, param_props_filter_expr, props_to_filter_expr};
use super::QueryPlanner;

impl<'s> QueryPlanner<'s> {
    // ── MATCH planning ────────────────────────────────────────────────────────

    pub(crate) fn plan_match(&self, mc: &MatchClause) -> Result<Operator, GqlError> {
        if mc.patterns.is_empty() {
            return Err(GqlError::Plan(
                crate::error::PlanError::ReturnStarNoVariables,
            ));
        }

        // Plan each pattern, then join them (multiple patterns = cross-product
        // via Apply for disconnected patterns).
        let mut pattern_ops: Vec<Operator> = mc
            .patterns
            .iter()
            .map(|p| self.plan_path_pattern(p))
            .collect::<Result<Vec<_>, _>>()?;

        let mut op = pattern_ops.remove(0);
        for other in pattern_ops {
            op = Operator::Apply {
                outer: Box::new(op),
                inner: Box::new(other),
            };
        }

        // Append WHERE clause from MatchClause if present.
        if let Some(pred) = &mc.where_clause {
            op = Operator::Filter {
                predicate: pred.clone(),
                child: Box::new(op),
            };
        }

        Ok(op)
    }

    /// Plan an existential subquery: `EXISTS { patterns [WHERE predicate] }`.
    ///
    /// Called by the executor when evaluating `Expr::ExistsSubquery` at runtime.
    /// Returns an operator that produces rows if the patterns match.
    pub fn plan_exists_subquery(
        &self,
        patterns: &[PathPattern],
        where_clause: Option<&Expr>,
    ) -> Result<Operator, GqlError> {
        if patterns.is_empty() {
            return Ok(Operator::SingleRow);
        }
        let mut pattern_ops: Vec<Operator> = patterns
            .iter()
            .map(|p| self.plan_path_pattern(p))
            .collect::<Result<Vec<_>, _>>()?;
        let mut op = pattern_ops.remove(0);
        for other in pattern_ops {
            op = Operator::Apply {
                outer: Box::new(op),
                inner: Box::new(other),
            };
        }
        if let Some(pred) = where_clause {
            op = Operator::Filter {
                predicate: pred.clone(),
                child: Box::new(op),
            };
        }
        Ok(op)
    }

    /// Plan a single `PathPattern` into nested `NodeScan` + `Expand` operators.
    ///
    /// When the pattern has a named path variable (e.g. `p = (a)-[r]->(b)`),
    /// wraps the result with an `AssemblePath` operator that collects node and
    /// edge IDs into a `Value::Path`.
    ///
    /// When `shortest_path` is set, generates a `ShortestPath` operator instead.
    fn plan_path_pattern(&self, pattern: &PathPattern) -> Result<Operator, GqlError> {
        // Handle shortestPath / allShortestPaths pattern functions.
        if let Some(sp_mode) = &pattern.shortest_path {
            return self.plan_shortest_path_pattern(pattern, *sp_mode);
        }

        let has_path_var = pattern.variable.is_some();
        let start = &pattern.start;
        let start_alias = start
            .variable
            .clone()
            .unwrap_or_else(|| Identifier::new("__anon0__"));
        let start_label = start.labels.first().cloned();

        // Track node/edge aliases for path assembly when a path variable is present.
        let mut path_node_aliases: Vec<Identifier> = Vec::new();
        let mut path_edge_aliases: Vec<Identifier> = Vec::new();
        if has_path_var {
            path_node_aliases.push(start_alias.clone());
        }

        // Build the node scan for the first node.
        // Only emit IndexScan when a declared property index exists.
        // Without a declared index the store cannot guarantee selective
        // lookups, so we fall back to NodeScan + Filter which is honest
        // about its O(N) cost and properly tracked by the row budget.
        let mut index_consumed_prop: Option<smol_str::SmolStr> = None;
        let start_map_props = match &start.properties {
            Some(Properties::Map(m)) => Some(m.as_slice()),
            _ => None,
        };
        let mut op = if let (Some(label), Some(props)) = (&start_label, start_map_props) {
            let selected_prop = self
                .store
                .list_indexes(self.graph)
                .ok()
                .and_then(|indexes| {
                    props.iter().find(|(prop_key, _)| {
                        indexes.iter().any(|idx| {
                            idx.label == label.as_str() && idx.property == prop_key.as_str()
                        })
                    })
                });
            if let Some((prop_key, prop_val)) = selected_prop {
                index_consumed_prop = Some(prop_key.clone());
                Operator::IndexScan {
                    label: label.clone(),
                    property: prop_key.to_string(),
                    value: Some(prop_val.clone()),
                    alias: start_alias.clone(),
                }
            } else {
                self.plan_node_scan(start_alias.clone(), start_label.clone())
            }
        } else {
            self.plan_node_scan(start_alias.clone(), start_label.clone())
        };

        // Wrap with a property filter for any inline properties that weren't
        // consumed by an IndexScan above.
        match &start.properties {
            Some(Properties::Map(props)) => {
                let remaining: Vec<_> = props
                    .iter()
                    .filter(|(k, _)| {
                        index_consumed_prop
                            .as_ref()
                            .map(|consumed| consumed != k)
                            .unwrap_or(true)
                    })
                    .cloned()
                    .collect();
                if !remaining.is_empty() {
                    let pred = props_to_filter_expr(&start_alias, &remaining);
                    op = Operator::Filter {
                        predicate: pred,
                        child: Box::new(op),
                    };
                }
            }
            Some(Properties::Parameter(name)) => {
                let pred = param_props_filter_expr(&start_alias, name);
                op = Operator::Filter {
                    predicate: pred,
                    child: Box::new(op),
                };
            }
            None => {}
        }

        // Add label filters for ALL labels beyond the first (which NodeScan handles).
        // Cypher semantics: (n:A:B:C) matches nodes with ALL of labels A, B, C.
        if start.labels.len() > 1 {
            for extra_label in &start.labels[1..] {
                let pred = label_filter_expr(&start_alias, extra_label);
                op = Operator::Filter {
                    predicate: pred,
                    child: Box::new(op),
                };
            }
        }

        // Inline WHERE clause on the start node pattern.
        if let Some(pred) = &start.where_clause {
            op = Operator::Filter {
                predicate: pred.clone(),
                child: Box::new(op),
            };
        }

        // Track the most recent src alias for multi-hop paths.
        let mut current_src = start_alias.clone();

        // Process each step (rel + node) in the path.
        for (step_idx, step) in pattern.steps.iter().enumerate() {
            let PathStep { rel, node } = step;
            let dst_alias = node
                .variable
                .clone()
                .unwrap_or_else(|| Identifier::new(format!("__anon{}__", step_idx + 1)));

            // Variable-length expand.
            if let Some(range) = &rel.range {
                let (min, max) =
                    crate::planner::validate_varlen_bounds(range.min.unwrap_or(1), range.max)?;
                let path_alias =
                    if has_path_var {
                        // Ensure a path alias exists for VarLenExpand when tracking a named path.
                        Some(rel.variable.clone().unwrap_or_else(|| {
                            Identifier::new(format!("__path_rel_{}__", step_idx))
                        }))
                    } else {
                        rel.variable.clone()
                    };
                // For VarLenExpand with a named path, the VarLenExpand itself
                // produces a Path value; the AssemblePath wrapper will merge it.
                if has_path_var {
                    if let Some(ref pa) = path_alias {
                        path_edge_aliases.push(pa.clone());
                    }
                    path_node_aliases.push(dst_alias.clone());
                }
                // Inline property constraints on a variable-length relationship
                // must be checked for every edge in the path during BFS traversal.
                // We pass them as `rel_props` using the sentinel alias `__varlen_edge__`.
                let rel_props_expr = match &rel.properties {
                    Some(Properties::Map(props)) if !props.is_empty() => Some(
                        props_to_filter_expr(&Identifier::new("__varlen_edge__"), props),
                    ),
                    Some(Properties::Parameter(name)) => Some(param_props_filter_expr(
                        &Identifier::new("__varlen_edge__"),
                        name,
                    )),
                    _ => None,
                };
                op = Operator::VarLenExpand {
                    direction: rel.direction,
                    rel_types: rel.rel_types.clone(),
                    min,
                    max,
                    src_alias: current_src.clone(),
                    path_alias,
                    path_mode: has_path_var,
                    dst_alias: dst_alias.clone(),
                    child: Box::new(op),
                    rel_props: rel_props_expr,
                };
            } else {
                // Single-hop expand.
                let rel_alias = Some(
                    rel.variable
                        .clone()
                        .unwrap_or_else(|| Identifier::new(format!("__anon_rel_{}__", step_idx))),
                );
                if has_path_var {
                    if let Some(ref ra) = rel_alias {
                        path_edge_aliases.push(ra.clone());
                    }
                    path_node_aliases.push(dst_alias.clone());
                }
                op = Operator::Expand {
                    direction: rel.direction,
                    rel_types: rel.rel_types.clone(),
                    src_alias: current_src.clone(),
                    rel_alias,
                    dst_alias: dst_alias.clone(),
                    child: Box::new(op),
                };
            }

            // If the relationship has inline property constraints, add a property filter.
            match &rel.properties {
                Some(Properties::Map(props)) if !props.is_empty() && rel.range.is_none() => {
                    let filter_alias = rel
                        .variable
                        .clone()
                        .unwrap_or_else(|| Identifier::new(format!("__anon_rel_{}__", step_idx)));
                    let pred = props_to_filter_expr(&filter_alias, props);
                    op = Operator::Filter {
                        predicate: pred,
                        child: Box::new(op),
                    };
                }
                Some(Properties::Parameter(name)) if rel.range.is_none() => {
                    let filter_alias = rel
                        .variable
                        .clone()
                        .unwrap_or_else(|| Identifier::new(format!("__anon_rel_{}__", step_idx)));
                    let pred = param_props_filter_expr(&filter_alias, name);
                    op = Operator::Filter {
                        predicate: pred,
                        child: Box::new(op),
                    };
                }
                _ => {}
            }

            // Inline WHERE clause on the relationship pattern.
            if let Some(pred) = &rel.where_clause {
                op = Operator::Filter {
                    predicate: pred.clone(),
                    child: Box::new(op),
                };
            }

            // If the destination node has label constraints, add label filters.
            for dst_label in &node.labels {
                let pred = label_filter_expr(&dst_alias, dst_label);
                op = Operator::Filter {
                    predicate: pred,
                    child: Box::new(op),
                };
            }

            // If the destination node has property constraints, add a property filter.
            match &node.properties {
                Some(Properties::Map(props)) if !props.is_empty() => {
                    let pred = props_to_filter_expr(&dst_alias, props);
                    op = Operator::Filter {
                        predicate: pred,
                        child: Box::new(op),
                    };
                }
                Some(Properties::Parameter(name)) => {
                    let pred = param_props_filter_expr(&dst_alias, name);
                    op = Operator::Filter {
                        predicate: pred,
                        child: Box::new(op),
                    };
                }
                _ => {}
            }

            // Inline WHERE clause on the destination node pattern.
            if let Some(pred) = &node.where_clause {
                op = Operator::Filter {
                    predicate: pred.clone(),
                    child: Box::new(op),
                };
            }

            // Update current_src for multi-hop patterns.
            current_src = dst_alias;
        }

        // If the pattern has a named path variable, wrap with AssemblePath.
        if let Some(path_var) = &pattern.variable {
            op = Operator::AssemblePath {
                path_alias: path_var.clone(),
                node_aliases: path_node_aliases,
                edge_aliases: path_edge_aliases,
                child: Box::new(op),
            };
        }

        Ok(op)
    }

    pub(crate) fn plan_node_scan(&self, alias: Identifier, label: Option<Label>) -> Operator {
        Operator::NodeScan { label, alias }
    }

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
    fn plan_shortest_path_pattern(
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
