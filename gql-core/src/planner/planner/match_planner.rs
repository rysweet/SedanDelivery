//! MATCH and OPTIONAL MATCH planning — pattern compilation and edge expansion.

use crate::ast::{Expr, Identifier, Label, MatchClause, PathPattern, PathStep, Properties};
use crate::error::GqlError;
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
    fn plan_path_pattern(&self, pattern: &PathPattern) -> Result<Operator, GqlError> {
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
        // If the node pattern has a label and a single equality property filter,
        // check whether a matching index exists. If so, emit IndexScan directly
        // (skipping the post-filter); otherwise fall back to NodeScan + Filter.
        let mut index_consumed_prop: Option<smol_str::SmolStr> = None;
        let start_map_props = match &start.properties {
            Some(Properties::Map(m)) => Some(m.as_slice()),
            _ => None,
        };
        let mut op = if let (Some(label), Some(props)) = (&start_label, start_map_props) {
            // Look for a single-property equality that maps to a literal value.
            // We take the first one that has an index; remaining props become a Filter.
            let indexes = self.store.list_indexes(self.graph).unwrap_or_default();
            let indexed_prop = props.iter().find(|(prop_key, _val)| {
                indexes
                    .iter()
                    .any(|idx| idx.label == label.as_str() && idx.property == prop_key.as_str())
            });
            if let Some((prop_key, _)) = indexed_prop {
                index_consumed_prop = Some(prop_key.clone());
                Operator::IndexScan {
                    label: label.clone(),
                    property: prop_key.to_string(),
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
                // Always assign a rel_alias so that the edge ID is stored in the
                // row, which enables edge isomorphism checking across multi-hop
                // patterns (openCypher spec: each relationship can only be bound
                // once per path traversal). For named paths and inline properties
                // we also need the alias for path assembly / filtering.
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
            // For variable-length patterns, this is handled inside VarLenExpand via
            // rel_props (each traversed edge is checked during BFS), so skip the Filter.
            match &rel.properties {
                Some(Properties::Map(props)) if !props.is_empty() && rel.range.is_none() => {
                    // rel_alias is guaranteed to exist because we always assign one above.
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

            // If the destination node has label constraints, add label filters for ALL labels.
            // Cypher semantics: (n:A:B:C) matches nodes with ALL of labels A, B, C.
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
}
