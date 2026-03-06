//! Mutation planning — CREATE, DELETE, SET, REMOVE, MERGE.

use crate::ast::{
    CreateClause, DeleteClause, Identifier, MergeClause, PathStep, RemoveClause, SetClause,
};
use crate::error::{GqlError, PlanError};
use crate::planner::{CreatePattern, Operator, RemoveItem, SetItem};

use super::helpers::{
    map_remove_clause_item, map_set_clause_item, properties_to_prop_pairs, PathBinding,
};
use super::QueryPlanner;

impl<'s> QueryPlanner<'s> {
    // ── CREATE planning ───────────────────────────────────────────────────────

    pub(crate) fn plan_create(
        &self,
        cc: &CreateClause,
        child: Option<Operator>,
    ) -> Result<Operator, GqlError> {
        let mut patterns = Vec::new();
        let mut anon_counter: usize = 0;

        // Collect path variable info per pattern so we can wrap with
        // AssemblePath after building the Create operator.
        let mut path_vars: Vec<Option<PathBinding>> = Vec::new();

        for (pat_idx, path_pattern) in cc.patterns.iter().enumerate() {
            let mut nodes: Vec<crate::planner::CreateNodeSpec> = Vec::new();
            let mut edges: Vec<crate::planner::CreateEdgeSpec> = Vec::new();
            let mut path_node_aliases: Vec<Identifier> = Vec::new();
            let mut path_edge_aliases: Vec<Identifier> = Vec::new();

            // Extract nodes from the pattern.
            // Generate unique aliases for anonymous nodes so the executor can
            // track them across node creation and edge creation.
            let start = &path_pattern.start;
            let start_alias = start.variable.clone().unwrap_or_else(|| {
                anon_counter += 1;
                Identifier::new(format!("__anon_{pat_idx}_{anon_counter}__"))
            });
            let node_props = properties_to_prop_pairs(start.properties.as_ref());
            nodes.push((Some(start_alias.clone()), start.labels.clone(), node_props));
            path_node_aliases.push(start_alias.clone());

            // Track the resolved alias for the previous node in the path.
            let mut prev_alias = start_alias.clone();

            for (step_idx, step) in path_pattern.steps.iter().enumerate() {
                let PathStep { rel, node } = step;

                // Destination node — generate alias if anonymous.
                let dst_alias = node.variable.clone().unwrap_or_else(|| {
                    anon_counter += 1;
                    Identifier::new(format!("__anon_{pat_idx}_{anon_counter}__"))
                });
                let dst_props = properties_to_prop_pairs(node.properties.as_ref());
                nodes.push((Some(dst_alias.clone()), node.labels.clone(), dst_props));
                path_node_aliases.push(dst_alias.clone());

                // Edge specification.
                let rel_type = rel.rel_types.first().ok_or_else(|| {
                    GqlError::Plan(PlanError::UndefinedVariable {
                        name: format!("CREATE relationship at step {step_idx} has no type"),
                        offset: 0,
                    })
                })?;

                let src_for_edge = prev_alias.clone();
                let dst_for_edge = dst_alias.clone();

                let rel_props = properties_to_prop_pairs(rel.properties.as_ref());

                // When a path variable exists on the CREATE pattern, ensure
                // the edge has an alias so AssemblePath can look it up in the
                // row. Assign a synthetic alias if the user did not provide one.
                let edge_alias = if path_pattern.variable.is_some() {
                    Some(rel.variable.clone().unwrap_or_else(|| {
                        anon_counter += 1;
                        Identifier::new(format!("__create_rel_{pat_idx}_{step_idx}__"))
                    }))
                } else {
                    rel.variable.clone()
                };

                if let Some(ref ea) = edge_alias {
                    path_edge_aliases.push(ea.clone());
                }

                edges.push((
                    edge_alias,
                    src_for_edge,
                    dst_for_edge,
                    rel_type.clone(),
                    rel.direction,
                    rel_props,
                ));

                prev_alias = dst_alias;
            }

            patterns.push(CreatePattern { nodes, edges });

            // Record path variable info for wrapping with AssemblePath.
            if let Some(ref path_var) = path_pattern.variable {
                path_vars.push(Some((
                    path_var.clone(),
                    path_node_aliases,
                    path_edge_aliases,
                )));
            } else {
                path_vars.push(None);
            }
        }

        let mut op = Operator::Create {
            patterns,
            child: child.map(Box::new),
        };

        // Wrap with AssemblePath for each CREATE pattern that has a named path
        // variable, so `p` is available in subsequent RETURN / WITH clauses.
        for (path_alias, node_aliases, edge_aliases) in path_vars.into_iter().flatten() {
            op = Operator::AssemblePath {
                path_alias,
                node_aliases,
                edge_aliases,
                child: Box::new(op),
            };
        }

        Ok(op)
    }

    // ── DELETE planning ───────────────────────────────────────────────────────

    pub(crate) fn plan_delete(
        &self,
        dc: &DeleteClause,
        child: Option<Operator>,
    ) -> Result<Operator, GqlError> {
        let child = child.ok_or_else(|| {
            GqlError::Plan(PlanError::UndefinedVariable {
                name: "DELETE without preceding MATCH or WITH".to_string(),
                offset: 0,
            })
        })?;

        Ok(Operator::Delete {
            exprs: dc.exprs.clone(),
            detach: dc.detach,
            child: Box::new(child),
        })
    }

    // ── SET planning ──────────────────────────────────────────────────────────

    pub(crate) fn plan_set(
        &self,
        sc: &SetClause,
        child: Option<Operator>,
    ) -> Result<Operator, GqlError> {
        let child = child.ok_or_else(|| {
            GqlError::Plan(PlanError::UndefinedVariable {
                name: "SET without preceding MATCH or WITH".to_string(),
                offset: 0,
            })
        })?;

        let items: Vec<SetItem> = sc.items.iter().map(map_set_clause_item).collect();

        Ok(Operator::Set {
            items,
            child: Box::new(child),
        })
    }

    // ── REMOVE planning ───────────────────────────────────────────────────────

    pub(crate) fn plan_remove(
        &self,
        rc: &RemoveClause,
        child: Option<Operator>,
    ) -> Result<Operator, GqlError> {
        let child = child.ok_or_else(|| {
            GqlError::Plan(PlanError::UndefinedVariable {
                name: "REMOVE without preceding MATCH or WITH".to_string(),
                offset: 0,
            })
        })?;

        let items: Vec<RemoveItem> = rc.items.iter().map(map_remove_clause_item).collect();

        Ok(Operator::Remove {
            items,
            child: Box::new(child),
        })
    }

    // ── MERGE planning ────────────────────────────────────────────────────────

    pub(crate) fn plan_merge(
        &self,
        mc: &MergeClause,
        child: Option<Operator>,
    ) -> Result<Operator, GqlError> {
        // Check if this is a relationship merge: MERGE (a)-[r:TYPE]->(b)
        if !mc.pattern.steps.is_empty() {
            return self.plan_merge_relationship(mc, child);
        }

        let node_alias = mc.pattern.start.variable.clone();
        let labels = mc.pattern.start.labels.clone();
        let properties = properties_to_prop_pairs(mc.pattern.start.properties.as_ref());

        let on_create: Vec<SetItem> = mc.on_create.iter().map(map_set_clause_item).collect();
        let on_match: Vec<SetItem> = mc.on_match.iter().map(map_set_clause_item).collect();

        let merge_op = Operator::Merge {
            node_alias: node_alias.clone(),
            labels,
            properties,
            on_create,
            on_match,
            child: child.map(Box::new),
        };

        // If the MERGE clause has a path variable (e.g. `MERGE p = (a {...})`),
        // wrap with AssemblePath so `p` is available in subsequent RETURN.
        if let Some(ref path_var) = mc.pattern.variable {
            let node_aliases = node_alias.into_iter().collect();
            Ok(Operator::AssemblePath {
                path_alias: path_var.clone(),
                node_aliases,
                edge_aliases: vec![],
                child: Box::new(merge_op),
            })
        } else {
            Ok(merge_op)
        }
    }

    fn plan_merge_relationship(
        &self,
        mc: &MergeClause,
        child: Option<Operator>,
    ) -> Result<Operator, GqlError> {
        let child = child.ok_or_else(|| {
            GqlError::Plan(PlanError::UndefinedVariable {
                name: "MERGE relationship requires preceding MATCH".to_string(),
                offset: 0,
            })
        })?;

        let step = &mc.pattern.steps[0];
        let src_alias = mc
            .pattern
            .start
            .variable
            .clone()
            .unwrap_or_else(|| Identifier::new("__merge_src__"));
        let dst_alias = step
            .node
            .variable
            .clone()
            .unwrap_or_else(|| Identifier::new("__merge_dst__"));
        // When a path variable exists on the MERGE pattern, ensure we capture the
        // edge ID by assigning a synthetic alias if none was specified by the user.
        let rel_alias = if mc.pattern.variable.is_some() {
            Some(
                step.rel
                    .variable
                    .clone()
                    .unwrap_or_else(|| Identifier::new("__merge_rel__")),
            )
        } else {
            step.rel.variable.clone()
        };
        let rel_type = step
            .rel
            .rel_types
            .first()
            .cloned()
            .unwrap_or_else(|| crate::ast::RelType::new(""));
        let direction = step.rel.direction;
        let properties = properties_to_prop_pairs(step.rel.properties.as_ref());

        let on_create: Vec<SetItem> = mc.on_create.iter().map(map_set_clause_item).collect();
        let on_match: Vec<SetItem> = mc.on_match.iter().map(map_set_clause_item).collect();

        let merge_op = Operator::MergeRelationship {
            src_alias: src_alias.clone(),
            dst_alias: dst_alias.clone(),
            rel_alias: rel_alias.clone(),
            rel_type,
            direction,
            properties,
            on_create,
            on_match,
            child: Box::new(child),
        };

        // If the MERGE clause has a path variable (e.g. `MERGE p = (a)-[:R]->(b)`),
        // wrap with AssemblePath so `p` is available in subsequent RETURN.
        if let Some(ref path_var) = mc.pattern.variable {
            let node_aliases = vec![src_alias, dst_alias];
            // rel_alias is guaranteed Some here (forced above when path_var is Some).
            let edge_aliases = rel_alias.into_iter().collect();
            Ok(Operator::AssemblePath {
                path_alias: path_var.clone(),
                node_aliases,
                edge_aliases,
                child: Box::new(merge_op),
            })
        } else {
            Ok(merge_op)
        }
    }
}
