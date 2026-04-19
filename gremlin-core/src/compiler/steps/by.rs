//! `by` category step compilation arms extracted from `dispatch.rs`.
//!
//! Pure mechanical extraction — no behavior changes. Each arm body is
//! verbatim from the corresponding case in the original monolithic match.

#![allow(unused_imports)]

use std::collections::HashMap;

use cypher_core::ast::{BinaryOp, Direction, Expr, Identifier, Label, RelType};
use cypher_core::planner::{
    AggFunction, CreateEdgeSpec, CreatePattern, GremlinPop, Operator, SetItem,
};
use smol_str::SmolStr;

use super::super::compile_from_context::CompileFromContext;
use super::super::helpers::*;
use super::super::{CompileError, GremlinCompiler, SelectByModulator};
use super::StepFlow;
use crate::bytecode::{GremlinStep, Predicate};
use crate::types::GremlinValue;

impl GremlinCompiler {
    pub(super) fn compile_by_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::By(key, descending) => {
                if ctx.pending_math {
                    // by() after math() — resolve variable references with property key
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("math");
                    let mut args = vec![
                        Expr::Variable(ctx.pre_math_alias.clone()),
                        Expr::Str(ctx.pending_math_expr.clone().into_boxed_str()),
                    ];
                    // For each variable in the expression, resolve using the by key
                    for var_name in extract_math_variables(&ctx.pending_math_expr) {
                        if let Some(stacks) = self.label_stacks.get(&var_name) {
                            if let Some(label_alias) = stacks.last() {
                                args.push(Expr::Str(var_name.into_boxed_str()));
                                if key.is_empty() {
                                    args.push(Expr::Variable(label_alias.clone()));
                                } else {
                                    args.push(Expr::Property(
                                        Box::new(Expr::Variable(label_alias.clone())),
                                        SmolStr::new(key),
                                    ));
                                }
                            }
                        } else if let Some(se_ident) = self.alias_map.get(&var_name) {
                            args.push(Expr::Str(var_name.into_boxed_str()));
                            if key.is_empty() {
                                args.push(Expr::Variable(se_ident.clone()));
                            } else {
                                args.push(Expr::Property(
                                    Box::new(Expr::Variable(se_ident.clone())),
                                    SmolStr::new(key),
                                ));
                            }
                        }
                    }
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_math"),
                            args,
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    ctx.current_alias = alias;
                    ctx.pending_math = false;
                    ctx.pending_math_expr.clear();
                } else if ctx.pending_order && ctx.pending_order_local {
                    // by() after order(Scope.local) — sort within list by key
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("srtl");
                    let by_key = if key.is_empty() { "" } else { key };
                    let desc_str = if *descending { "desc" } else { "asc" };
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_sort_local"),
                            args: vec![
                                Expr::Variable(ctx.current_alias.clone()),
                                Expr::Str(by_key.to_string().into_boxed_str()),
                                Expr::Str(desc_str.to_string().into_boxed_str()),
                            ],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    ctx.current_alias = alias;
                    ctx.pending_order = false;
                    ctx.pending_order_local = false;
                } else if ctx.pending_order {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    if key.is_empty() {
                        // by() with no key — sort by ctx.current value
                        ctx.current = Some(Operator::Sort {
                            items: vec![(Expr::Variable(ctx.current_alias.clone()), !descending)],
                            gremlin_order: true,
                            child: Box::new(child),
                        });
                    } else if key == "~id" || key == "T.id" || key == "t[id]" {
                        let sort_expr = Expr::FunctionCall {
                            name: SmolStr::new("id"),
                            args: vec![Expr::Variable(ctx.current_alias.clone())],
                            distinct: false,
                        };
                        ctx.current = Some(Operator::Sort {
                            items: vec![(sort_expr, !descending)],
                            gremlin_order: true,
                            child: Box::new(child),
                        });
                    } else if key == "~label" || key == "T.label" || key == "t[label]" {
                        let sort_expr = Expr::FunctionCall {
                            name: SmolStr::new("labels"),
                            args: vec![Expr::Variable(ctx.current_alias.clone())],
                            distinct: false,
                        };
                        ctx.current = Some(Operator::Sort {
                            items: vec![(sort_expr, !descending)],
                            gremlin_order: true,
                            child: Box::new(child),
                        });
                    } else if key == "~key" || key == "T.key" || key == "t[key]" {
                        // Sort by property key name (for VertexProperty traversers)
                        let sort_expr = Expr::FunctionCall {
                            name: SmolStr::new("gremlin_property_key"),
                            args: vec![Expr::Variable(ctx.current_alias.clone())],
                            distinct: false,
                        };
                        ctx.current = Some(Operator::Sort {
                            items: vec![(sort_expr, !descending)],
                            gremlin_order: true,
                            child: Box::new(child),
                        });
                    } else if key == "~value" || key == "T.value" || key == "t[value]" {
                        // Sort by property value (for VertexProperty traversers)
                        let sort_expr = Expr::FunctionCall {
                            name: SmolStr::new("gremlin_property_value"),
                            args: vec![Expr::Variable(ctx.current_alias.clone())],
                            distinct: false,
                        };
                        ctx.current = Some(Operator::Sort {
                            items: vec![(sort_expr, !descending)],
                            gremlin_order: true,
                            child: Box::new(child),
                        });
                    } else if key == "Column.values" {
                        // Sort by map entry values (for unfold'd single-entry Maps)
                        let sort_expr = Expr::FunctionCall {
                            name: SmolStr::new("gremlin_column_values"),
                            args: vec![Expr::Variable(ctx.current_alias.clone())],
                            distinct: false,
                        };
                        ctx.current = Some(Operator::Sort {
                            items: vec![(sort_expr, !descending)],
                            gremlin_order: true,
                            child: Box::new(child),
                        });
                    } else if key == "Column.keys" {
                        // Sort by map entry keys (for unfold'd single-entry Maps)
                        let sort_expr = Expr::FunctionCall {
                            name: SmolStr::new("gremlin_column_keys"),
                            args: vec![Expr::Variable(ctx.current_alias.clone())],
                            distinct: false,
                        };
                        ctx.current = Some(Operator::Sort {
                            items: vec![(sort_expr, !descending)],
                            gremlin_order: true,
                            child: Box::new(child),
                        });
                    } else {
                        // Property-based sort
                        let prop_expr = Expr::Property(
                            Box::new(Expr::Variable(ctx.current_alias.clone())),
                            SmolStr::new(key),
                        );
                        if self.productive_by {
                            // ProductiveByStrategy: use coalesce to return Null
                            // for missing properties instead of filtering.
                            let safe_prop = Expr::FunctionCall {
                                name: SmolStr::new("coalesce"),
                                args: vec![prop_expr, Expr::Null],
                                distinct: false,
                            };
                            ctx.current = Some(Operator::Sort {
                                items: vec![(safe_prop, !descending)],
                                gremlin_order: true,
                                child: Box::new(child),
                            });
                        } else {
                            // Filter out elements missing the property
                            let filtered = Operator::Filter {
                                predicate: Expr::IsNotNull(Box::new(prop_expr.clone())),
                                child: Box::new(child),
                            };
                            ctx.current = Some(Operator::Sort {
                                items: vec![(prop_expr, !descending)],
                                gremlin_order: true,
                                child: Box::new(filtered),
                            });
                        }
                    }
                    ctx.pending_order = false;
                } else if !ctx.pending_order && matches!(ctx.current, Some(Operator::Sort { .. })) {
                    // Additional by() after a Sort — add secondary sort key
                    let sort_expr = if key.is_empty() {
                        Expr::Variable(ctx.current_alias.clone())
                    } else {
                        let prop = Expr::Property(
                            Box::new(Expr::Variable(ctx.current_alias.clone())),
                            SmolStr::new(key),
                        );
                        if self.productive_by {
                            Expr::FunctionCall {
                                name: SmolStr::new("coalesce"),
                                args: vec![prop, Expr::Null],
                                distinct: false,
                            }
                        } else {
                            prop
                        }
                    };
                    if let Some(Operator::Sort { ref mut items, .. }) = ctx.current {
                        items.push((sort_expr, !descending));
                    }
                } else if ctx.pending_group {
                    // by() after group()/groupCount() — modifies GremlinGroup.
                    // Use ctx.pre_group_alias (the alias before group() changed it).
                    if ctx.pending_group_by_count == 0 {
                        // First by() = group key
                        if !key.is_empty() {
                            // A non-empty key means the group key is no longer the identity.
                            if let Some(Operator::GremlinGroup {
                                ref mut key_is_identity,
                                ..
                            }) = ctx.current
                            {
                                *key_is_identity = false;
                            }
                            // Try parsing __.prefix strings as sub-traversals
                            if key.starts_with("__.") && key.contains('(') {
                                let no_param = |_: &str| -> Option<GremlinValue> { None };
                                if let Ok(sub_steps) =
                                    crate::text_parser::parse_inner_traversal(key, &no_param)
                                {
                                    if !sub_steps.is_empty() {
                                        let mut sub = self.sub_compiler();
                                        if let Ok(sub_op) =
                                            sub.compile_seeded(&sub_steps, &ctx.pre_group_alias)
                                        {
                                            let sub_out_alias = sub.last_alias.clone();
                                            self.absorb(&sub);
                                            if let Some(Operator::GremlinGroup {
                                                ref mut key_traversal,
                                                ..
                                            }) = ctx.current
                                            {
                                                *key_traversal =
                                                    Some((Box::new(sub_op), sub_out_alias));
                                            }
                                        }
                                    }
                                }
                            } else if let Some(Operator::GremlinGroup {
                                ref mut key_expr, ..
                            }) = ctx.current
                            {
                                *key_expr = make_by_expr(key, &ctx.pre_group_alias);
                            }
                        }
                        ctx.pending_group_by_count = 1;
                    } else if ctx.pending_group_is_count {
                        // groupCount() only supports one by() modulator
                        return Err(CompileError::InvalidSequence(
                            "GroupCount step can only have one by modulator".into(),
                        ));
                    } else {
                        // Second by() = value aggregation
                        if key == "count()" || key == "__.count()" {
                            if let Some(Operator::GremlinGroup { ref mut agg_fn, .. }) = ctx.current
                            {
                                *agg_fn = AggFunction::Count;
                            }
                        } else if key == "fold()" || key == "__.fold()" {
                            if let Some(Operator::GremlinGroup {
                                ref mut agg_fn,
                                ref mut explicit_fold,
                                ..
                            }) = ctx.current
                            {
                                *agg_fn = AggFunction::Collect;
                                *explicit_fold = true;
                            }
                        } else if key == "sum()" || key == "__.sum()" {
                            if let Some(Operator::GremlinGroup { ref mut agg_fn, .. }) = ctx.current
                            {
                                *agg_fn = AggFunction::Sum;
                            }
                        } else if key.is_empty() {
                            // by() with no args — identity (collect the element)
                            if let Some(Operator::GremlinGroup { ref mut agg_fn, .. }) = ctx.current
                            {
                                *agg_fn = AggFunction::Collect;
                            }
                        } else if key.starts_with("__.") && key.contains('(') {
                            // __.prefix value-by: parse as sub-traversal
                            let no_param = |_: &str| -> Option<GremlinValue> { None };
                            if let Ok(sub_steps) =
                                crate::text_parser::parse_inner_traversal(key, &no_param)
                            {
                                if !sub_steps.is_empty() {
                                    let (proj_steps, trailing_agg) =
                                        split_trailing_group_value_agg(&sub_steps);
                                    if proj_steps.is_empty() {
                                        if let Some(Operator::GremlinGroup {
                                            ref mut agg_fn,
                                            ref mut explicit_fold,
                                            ..
                                        }) = ctx.current
                                        {
                                            if let Some(agg) = trailing_agg {
                                                if matches!(agg, AggFunction::Collect) {
                                                    *explicit_fold = true;
                                                }
                                                *agg_fn = agg;
                                            }
                                        }
                                    } else {
                                        let mut sub = self.sub_compiler();
                                        if let Ok(sub_op) =
                                            sub.compile_seeded(proj_steps, &ctx.pre_group_alias)
                                        {
                                            let sub_out_alias = sub.last_alias.clone();
                                            self.absorb(&sub);
                                            if let Some(Operator::GremlinGroup {
                                                ref mut value_traversal,
                                                ref mut agg_fn,
                                                ref mut explicit_fold,
                                                ..
                                            }) = ctx.current
                                            {
                                                *value_traversal =
                                                    Some((Box::new(sub_op), sub_out_alias));
                                                if let Some(agg) = trailing_agg {
                                                    if matches!(agg, AggFunction::Collect) {
                                                        *explicit_fold = true;
                                                    }
                                                    *agg_fn = agg;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        } else if let Some(Operator::GremlinGroup {
                            ref mut value_expr, ..
                        }) = ctx.current
                        {
                            // by("propName") — collect property values
                            *value_expr = make_by_expr(key, &ctx.pre_group_alias);
                        }
                        ctx.pending_group = false;
                    }
                } else if ctx.pending_select {
                    // by() after select() — normalize traversal-form
                    // modulators like by(__.math("_+_")) into compact keys.
                    if key.starts_with("__.") && key.contains('(') {
                        let no_param = |_: &str| -> Option<GremlinValue> { None };
                        if let Ok(sub_steps) =
                            crate::text_parser::parse_inner_traversal(key, &no_param)
                        {
                            ctx.select_by_mods
                                .push(SelectByModulator::Key(extract_by_traversal_key(&sub_steps)));
                        } else {
                            ctx.select_by_mods.push(SelectByModulator::Key(key.clone()));
                        }
                    } else {
                        ctx.select_by_mods.push(SelectByModulator::Key(key.clone()));
                    }
                } else if ctx.pending_path {
                    // by() after path() — collect by-key for path element transformation
                    ctx.path_by_keys.push(key.to_string());
                } else if ctx.pending_dedup {
                    if ctx.dedup_by_consumed {
                        return Err(CompileError::InvalidSequence(
                            "Dedup step can only have one by modulator".into(),
                        ));
                    }
                    // by() after dedup() — dedup by specific property
                    if !key.is_empty() {
                        if let Some(Operator::Distinct { ref mut by_key, .. }) = ctx.current {
                            if let Some(ref existing) = *by_key {
                                if existing.contains('\x1f') {
                                    // Multi-key dedup: apply property to each label
                                    let new_key = existing
                                        .split('\x1f')
                                        .map(|l| format!("__dedup_prop__{}__{}", key, l))
                                        .collect::<Vec<_>>()
                                        .join("\x1f");
                                    *by_key = Some(new_key);
                                } else {
                                    *by_key = Some(format!("__dedup_prop__{}__{}", key, existing));
                                }
                            } else {
                                *by_key =
                                    Some(format!("__dedup_prop__{}__{}", key, ctx.current_alias.0));
                            }
                        }
                    }
                    ctx.dedup_by_consumed = true;
                } else if ctx.pending_aggregate {
                    // by() after aggregate() — project a property from each traverser
                    if let Some(Operator::GremlinAggregate {
                        ref mut by_keys, ..
                    }) = ctx.current
                    {
                        if !key.is_empty() {
                            by_keys.push(key.to_string());
                        }
                    }
                    ctx.pending_aggregate = false;
                    ctx.aggregate_by_consumed = true;
                } else if ctx.pending_format {
                    // by(key) after format() — compile by-key to a property access
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let by_alias = self.next_alias("fby");
                    let by_expr = if key.is_empty() {
                        Expr::Variable(ctx.format_pre_alias.clone())
                    } else {
                        make_by_expr(key, &ctx.format_pre_alias)
                    };
                    let mut items = vec![(by_expr, Some(by_alias.clone()))];
                    items.extend(self.label_carry_items());
                    ctx.current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    ctx.format_by_aliases.push(by_alias);
                } else if ctx.pending_sack_op {
                    // by(key) after sack(op) — rewrite the sack Project to
                    // use a property/label expression instead of ctx.current_alias.
                    if !key.is_empty() {
                        let by_expr = make_by_expr(key, &ctx.current_alias);
                        if let Some(Operator::Project { ref mut items, .. }) = ctx.current {
                            // First item is the sack assignment; rewrite the rhs
                            // (ctx.current_alias reference) with the by expression.
                            if let Some((expr, _)) = items.first_mut() {
                                rewrite_sack_by(expr, &ctx.current_alias, &by_expr);
                            }
                        }
                        // Filter out traversers where the by-key property is null
                        // (Gremlin drops traversers when by() can't resolve a value).
                        let sack_var = Identifier::new("__sack");
                        if let Some(proj) = ctx.current.take() {
                            ctx.current = Some(Operator::Filter {
                                predicate: Expr::IsNotNull(Box::new(Expr::Variable(sack_var))),
                                child: Box::new(proj),
                            });
                        }
                    }
                    ctx.pending_sack_op = false;
                    ctx.pending_sack_by_consumed = true;
                } else if ctx.pending_project {
                    // by() after project() — replace the default property
                    // expression for the next project key with the by() expression.
                    if ctx.project_by_count < ctx.project_keys.len() {
                        // Check if the key looks like a sub-traversal
                        let looks_like_traversal = key.contains('(') && !key.is_empty();
                        if looks_like_traversal {
                            // Parse and compile the sub-traversal
                            let no_param = |_: &str| -> Option<GremlinValue> { None };
                            if let Ok(sub_steps) =
                                crate::text_parser::parse_inner_traversal(key, &no_param)
                            {
                                if !sub_steps.is_empty() {
                                    let mut sub = self.sub_compiler();
                                    if let Ok(sub_op) =
                                        sub.compile_seeded(&sub_steps, &ctx.pre_project_alias)
                                    {
                                        let sub_out_alias = sub.last_alias.clone();
                                        self.absorb(&sub);

                                        // Convert to GremlinProject
                                        match &ctx.current {
                                            Some(Operator::Project { items, child }) => {
                                                let mut entries: Vec<(
                                                    String,
                                                    Operator,
                                                    Identifier,
                                                    bool,
                                                )> = Vec::new();
                                                for (i, (expr, alias_opt)) in
                                                    items.iter().enumerate()
                                                {
                                                    if i >= ctx.project_keys.len() {
                                                        break;
                                                    }
                                                    if i == ctx.project_by_count {
                                                        // by(traversal) — not identity
                                                        entries.push((
                                                            ctx.project_keys[i].clone(),
                                                            sub_op.clone(),
                                                            sub_out_alias.clone(),
                                                            false,
                                                        ));
                                                    } else {
                                                        let prop_op = Operator::Project {
                                                            items: vec![(
                                                                expr.clone(),
                                                                alias_opt.clone(),
                                                            )],
                                                            child: Box::new(Operator::SingleRow),
                                                        };
                                                        // Prior entries may have been set by
                                                        // empty by() (identity). We can't
                                                        // recover that info here since it was
                                                        // stored in items[i].0 type. Assume
                                                        // non-identity for safety.
                                                        entries.push((
                                                            ctx.project_keys[i].clone(),
                                                            prop_op,
                                                            alias_opt
                                                                .as_ref()
                                                                .cloned()
                                                                .unwrap_or_else(|| {
                                                                    Identifier::new(
                                                                        &ctx.project_keys[i],
                                                                    )
                                                                }),
                                                            false,
                                                        ));
                                                    }
                                                }
                                                let proj_alias = self.next_alias("proj");
                                                ctx.current = Some(Operator::GremlinProject {
                                                    entries,
                                                    child: child.clone(),
                                                    current_alias: ctx.pre_project_alias.clone(),
                                                    out_alias: proj_alias,
                                                    productive: self.productive_by,
                                                });
                                            }
                                            Some(Operator::GremlinProject {
                                                entries: old_entries,
                                                child,
                                                current_alias: ca,
                                                out_alias,
                                                ..
                                            }) => {
                                                let mut entries = old_entries.clone();
                                                if ctx.project_by_count < entries.len() {
                                                    entries[ctx.project_by_count] = (
                                                        ctx.project_keys[ctx.project_by_count]
                                                            .clone(),
                                                        sub_op,
                                                        sub_out_alias,
                                                        false, // by(traversal) — not identity
                                                    );
                                                }
                                                ctx.current = Some(Operator::GremlinProject {
                                                    entries,
                                                    child: child.clone(),
                                                    current_alias: ca.clone(),
                                                    out_alias: out_alias.clone(),
                                                    productive: self.productive_by,
                                                });
                                            }
                                            _ => {}
                                        }
                                        ctx.project_by_count += 1;
                                        if ctx.project_by_count >= ctx.project_keys.len() {
                                            ctx.pending_project = false;
                                            if let Some(Operator::GremlinProject {
                                                out_alias: proj_alias,
                                                ..
                                            }) = &ctx.current
                                            {
                                                ctx.current_alias = proj_alias.clone();
                                            }
                                        }
                                        // Skip the default handling below
                                    } else {
                                        // Compilation failed — use simple property expression.
                                        // This is correct for by("propName") but may lose
                                        // semantics for complex by(traversal) modulations.
                                        let by_expr = make_by_expr(key, &ctx.pre_project_alias);
                                        if let Some(Operator::Project { ref mut items, .. }) =
                                            ctx.current
                                        {
                                            if let Some((expr, _)) =
                                                items.get_mut(ctx.project_by_count)
                                            {
                                                *expr = by_expr;
                                            }
                                        }
                                        ctx.project_by_count += 1;
                                        if ctx.project_by_count >= ctx.project_keys.len() {
                                            ctx.pending_project = false;
                                        }
                                    }
                                } else {
                                    // Empty by() steps — use simple property expression.
                                    let by_expr = make_by_expr(key, &ctx.pre_project_alias);
                                    if let Some(Operator::Project { ref mut items, .. }) =
                                        ctx.current
                                    {
                                        if let Some((expr, _)) = items.get_mut(ctx.project_by_count)
                                        {
                                            *expr = by_expr;
                                        }
                                    }
                                    ctx.project_by_count += 1;
                                    if ctx.project_by_count >= ctx.project_keys.len() {
                                        ctx.pending_project = false;
                                    }
                                }
                            } else {
                                // Parse failed — use simple property expression.
                                let by_expr = make_by_expr(key, &ctx.pre_project_alias);
                                if let Some(Operator::Project { ref mut items, .. }) = ctx.current {
                                    if let Some((expr, _)) = items.get_mut(ctx.project_by_count) {
                                        *expr = by_expr;
                                    }
                                }
                                ctx.project_by_count += 1;
                                if ctx.project_by_count >= ctx.project_keys.len() {
                                    ctx.pending_project = false;
                                }
                            }
                        } else {
                            // Simple property key
                            let by_expr = if key.is_empty() {
                                Expr::Variable(ctx.pre_project_alias.clone())
                            } else {
                                make_by_expr(key, &ctx.pre_project_alias)
                            };
                            if let Some(Operator::Project { ref mut items, .. }) = ctx.current {
                                if let Some((expr, _)) = items.get_mut(ctx.project_by_count) {
                                    *expr = by_expr;
                                }
                            } else if let Some(Operator::GremlinProject {
                                ref mut entries, ..
                            }) = ctx.current
                            {
                                // Already converted to GremlinProject — update the entry
                                if ctx.project_by_count < entries.len() {
                                    let is_identity = key.is_empty();
                                    let by_ex = if is_identity {
                                        Expr::Variable(ctx.pre_project_alias.clone())
                                    } else {
                                        make_by_expr(key, &ctx.pre_project_alias)
                                    };
                                    let prop_op = Operator::Project {
                                        items: vec![(
                                            by_ex,
                                            Some(Identifier::new(
                                                &ctx.project_keys[ctx.project_by_count],
                                            )),
                                        )],
                                        child: Box::new(Operator::SingleRow),
                                    };
                                    entries[ctx.project_by_count] = (
                                        ctx.project_keys[ctx.project_by_count].clone(),
                                        prop_op,
                                        Identifier::new(&ctx.project_keys[ctx.project_by_count]),
                                        is_identity,
                                    );
                                }
                            }
                            ctx.project_by_count += 1;
                            if ctx.project_by_count >= ctx.project_keys.len() {
                                ctx.pending_project = false;
                                // If GremlinProject, update ctx.current_alias to its out_alias
                                if let Some(Operator::GremlinProject {
                                    out_alias: ref proj_alias,
                                    ..
                                }) = ctx.current
                                {
                                    ctx.current_alias = proj_alias.clone();
                                }
                                // When all by() modulators are simple property keys, the
                                // operator stays as Operator::Project, which does NOT
                                // assemble the projected Map under ctx.current_alias.
                                // Add a map-assembly wrapper so downstream steps (e.g.
                                // merge(), combine()) receive a Map as the traverser
                                // instead of the original Node.  GremlinProject already
                                // handles this for traversal-based by().
                                if matches!(&ctx.current, Some(Operator::Project { .. }))
                                    && !ctx.project_keys.is_empty()
                                {
                                    let proj_alias = self.next_alias("proj");
                                    let map_expr = Expr::MapLiteral(
                                        ctx.project_keys
                                            .iter()
                                            .map(|k| {
                                                (
                                                    SmolStr::new(k.as_str()),
                                                    Expr::Variable(Identifier::new(k.as_str())),
                                                )
                                            })
                                            .collect(),
                                    );
                                    let inner = ctx.current.take().unwrap();
                                    // Carry existing label aliases first, then
                                    // place the map assembly last so it wins.
                                    let mut items = self.label_carry_items();
                                    items.push((map_expr, Some(proj_alias.clone())));
                                    ctx.current = Some(Operator::Project {
                                        items,
                                        child: Box::new(inner),
                                    });
                                    ctx.current_alias = proj_alias;
                                }
                            }
                        }
                    }
                } else if !ctx.pending_where_by_targets.is_empty() {
                    // by() after where() records a traversal-ring modulator
                    // and applies it only once the full by-list is known.
                    ctx.pending_where_by_modulators.push(key.to_string());
                } else if ctx.aggregate_by_consumed {
                    // Second by() after aggregate — error per Gremlin spec
                    return Err(CompileError::InvalidSequence(
                        "Aggregate step can only have one by modulator".into(),
                    ));
                } else if ctx.pending_sample && ctx.sample_by_consumed {
                    return Err(CompileError::InvalidSequence(
                        "Sample step can only have one by modulator".into(),
                    ));
                } else if ctx.pending_sack_by_consumed {
                    return Err(CompileError::InvalidSequence(
                        "Sack step can only have one by modulator".into(),
                    ));
                } else if ctx.pending_valuemap && ctx.valuemap_by_consumed {
                    return Err(CompileError::InvalidSequence(
                        "step can only have one by modulator".into(),
                    ));
                }
                // Track by() consumption for sample/sack/valueMap
                if ctx.pending_sample && !ctx.sample_by_consumed {
                    ctx.sample_by_consumed = true;
                    ctx.pending_sample_weight = Some(if key.is_empty() {
                        Expr::Variable(ctx.current_alias.clone())
                    } else {
                        make_by_expr(key, &ctx.current_alias)
                    });
                }
                if ctx.pending_sack_op && !ctx.pending_sack_by_consumed {
                    ctx.pending_sack_by_consumed = true;
                }
                if ctx.pending_valuemap && !ctx.valuemap_by_consumed {
                    ctx.valuemap_by_consumed = true;
                }
            }

            _ => unreachable!("compile_by_step received non-by variant: {:?}", step),
        }
        #[allow(unreachable_code)]
        Ok(StepFlow::Proceed)
    }
}
