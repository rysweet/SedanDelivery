//! `GremlinCompiler::compile_from` — the central per-step compilation routine.
//!
//! Extracted from `mod.rs` to keep that file focused on struct definition and
//! small helpers; this file holds the giant left-to-right step walker that
//! compiles a `Vec<GremlinStep>` into an `Operator` tree.

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::compile_from_context::CompileFromContext;
use super::helpers::*;
use super::steps::StepFlow;
use super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    /// Core compilation: walks steps left-to-right, wrapping each into the
    /// operator tree. `initial_op` / `initial_alias` allow callers to seed
    /// the compilation with an existing operator and alias.
    pub(super) fn compile_from(
        &mut self,
        steps: &[GremlinStep],
        initial_op: Option<Operator>,
        initial_alias: Identifier,
        initial_path_aliases: Vec<Identifier>,
    ) -> Result<Operator, CompileError> {
        if steps.is_empty() {
            return Ok(initial_op.unwrap_or(Operator::SingleRow));
        }

        // Preprocess infix and()/or() with no arguments into proper branching
        // forms — unless ConnectiveStrategy was explicitly disabled.
        let has_without_connective = steps
            .iter()
            .any(|s| matches!(s, GremlinStep::WithoutConnectiveStrategy));
        let preprocessed = if has_without_connective {
            steps.to_vec()
        } else {
            preprocess_infix_connectives(steps)
        };
        let steps = &preprocessed;

        // Pre-scan: detect if any step uses path tracking (path, simplePath, cyclicPath).
        // This enables carrying path aliases through Project operations.
        if !self.needs_path_tracking {
            self.needs_path_tracking = steps_need_path_tracking(steps);
        }
        // Pre-scan: detect if any select() uses Pop modifiers.
        if !self.needs_label_stacks {
            self.needs_label_stacks = steps_need_label_stacks(steps);
        }
        // Pre-scan: collect aggregate() keys so that within()/without() predicates
        // that reference a key defined LATER in the same step sequence (e.g. inside
        // repeat() bodies) can resolve to __gremlin_agg_<key> at compile time.
        collect_aggregate_keys_prescan(steps, &mut self.aggregate_keys);

        let mut ctx =
            CompileFromContext::new(initial_op, initial_alias.clone(), initial_path_aliases);
        // Sync path_tracking from initial_path_aliases
        self.path_tracking = ctx.path_aliases.clone();

        for step in steps {
            // Save alias before step to detect changes for path tracking
            let pre_step_alias = ctx.current_alias.clone();
            // Clear numeric subtype on value-changing steps; Is/As/filter steps preserve it.
            if !matches!(
                step,
                GremlinStep::Is(_)
                    | GremlinStep::As(_)
                    | GremlinStep::By(..)
                    | GremlinStep::ByTraversal(..)
            ) {
                self.numeric_subtype = None;
            }
            // When we leave the addE/mergeE modifier group (from/to/property/option),
            // fill in any __src/__dst placeholders that were never overridden.
            if ctx.adde_traverser.is_some()
                && !matches!(
                    step,
                    GremlinStep::From(_)
                        | GremlinStep::To(_)
                        | GremlinStep::Property(..)
                        | GremlinStep::Option(..)
                )
            {
                if let Some(ref traverser) = ctx.adde_traverser.take() {
                    if let Some(ref mut op) = ctx.current {
                        rewrite_create_alias(op, "__src", traverser);
                        rewrite_create_alias(op, "__dst", traverser);
                    }
                }
            }
            // Reset aggregate by-consumed flag on any non-By step
            if !matches!(step, GremlinStep::By(..) | GremlinStep::ByTraversal(..)) {
                ctx.aggregate_by_consumed = false;
                ctx.dedup_by_consumed = false;
                // dedup/group/aggregate/format by() modulators must immediately follow
                // their step. Any intervening non-By step terminates the pending state.
                ctx.pending_dedup = false;
                ctx.pending_aggregate = false;
                ctx.pending_format = false;
                ctx.pending_sack_op = false;
                ctx.pending_sack_by_consumed = false;
                ctx.pending_valuemap = false;
                ctx.valuemap_by_consumed = false;
                ctx.pending_group = false;
                ctx.pending_group_is_count = false;
                ctx.pending_group_by_count = 0;
                // Flush accumulated path by-keys into the GremlinPath/CyclicPath operator
                if ctx.pending_path && !ctx.path_by_keys.is_empty() {
                    match &mut ctx.current {
                        Some(Operator::GremlinPath {
                            ref mut by_keys, ..
                        }) => {
                            *by_keys = ctx.path_by_keys.clone();
                        }
                        Some(Operator::GremlinCyclicPath {
                            ref mut by_keys, ..
                        }) => {
                            *by_keys = ctx.path_by_keys.clone();
                        }
                        Some(Operator::GremlinSimplePath {
                            ref mut by_keys, ..
                        }) => {
                            *by_keys = ctx.path_by_keys.clone();
                        }
                        _ => {}
                    }
                }
                // Preserve ctx.pending_path through from()/to() modifiers so
                // subsequent by() modulations are still accumulated.
                let is_path_from_to = matches!(step, GremlinStep::From(_) | GremlinStep::To(_))
                    && matches!(
                        &ctx.current,
                        Some(Operator::GremlinPath { .. })
                            | Some(Operator::GremlinSimplePath { .. })
                            | Some(Operator::GremlinCyclicPath { .. })
                    );
                if !is_path_from_to {
                    ctx.pending_path = false;
                    ctx.path_by_keys.clear();
                }
                ctx.pending_project = false;
                if !ctx.pending_where_by_targets.is_empty()
                    && !ctx.pending_where_by_modulators.is_empty()
                {
                    if let Some(Operator::Filter {
                        ref mut predicate, ..
                    }) = ctx.current
                    {
                        finalize_where_by_modulation(
                            predicate,
                            &ctx.pending_where_by_targets,
                            &ctx.pending_where_by_modulators,
                            self.productive_by,
                        );
                    }
                }
                ctx.pending_where_by_targets.clear();
                ctx.pending_where_by_modulators.clear();
                // Flush pending math without by() modulation — resolve side-effect variables
                if ctx.pending_math {
                    let child = ctx.current.take().unwrap_or(Operator::SingleRow);
                    let alias = self.next_alias("math");
                    let mut args = vec![
                        Expr::Variable(ctx.pre_math_alias.clone()),
                        Expr::Str(ctx.pending_math_expr.clone().into_boxed_str()),
                    ];
                    // Resolve variable references in the expression from side effects
                    for var_name in extract_math_variables(&ctx.pending_math_expr) {
                        let se_key = format!("__side_effect_{}", var_name);
                        if let Some(se_ident) = self.alias_map.get(&var_name) {
                            args.push(Expr::Str(var_name.into_boxed_str()));
                            args.push(Expr::Variable(se_ident.clone()));
                        } else if self.side_effect_values.contains_key(&var_name) {
                            args.push(Expr::Str(var_name.into_boxed_str()));
                            args.push(Expr::Variable(Identifier::new(se_key)));
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
                }
                // Flush pending order: order() without by() → sort by identity
                if ctx.pending_order && !ctx.pending_order_local {
                    let child = ctx.current.take().unwrap_or(Operator::SingleRow);
                    ctx.current = Some(Operator::Sort {
                        items: vec![(Expr::Variable(ctx.current_alias.clone()), true)],
                        gremlin_order: true,
                        child: Box::new(child),
                    });
                    ctx.pending_order = false;
                }
                // Flush pending order(Scope.local) without by() — sort list by natural ordering
                if ctx.pending_order && ctx.pending_order_local {
                    let child = ctx.current.take().unwrap_or(Operator::SingleRow);
                    let alias = self.next_alias("srtl");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_sort_local"),
                            args: vec![
                                Expr::Variable(ctx.current_alias.clone()),
                                Expr::Str("".into()),
                                Expr::Str("asc".into()),
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
                }
                // Finalize pending sample once the optional by() window has closed.
                if ctx.pending_sample {
                    let child = ctx.current.take().unwrap_or(Operator::SingleRow);
                    ctx.current = Some(Operator::GremlinSample {
                        count: ctx.pending_sample_count as usize,
                        weight: ctx.pending_sample_weight.clone(),
                        seed: self.seed,
                        child: Box::new(child),
                    });
                    ctx.pending_sample = false;
                    ctx.sample_by_consumed = false;
                    ctx.pending_sample_count = 0;
                    ctx.pending_sample_weight = None;
                }
                // Finalize pending format with collected by() modulations
                if ctx.pending_format && !ctx.format_by_aliases.is_empty() {
                    let child = ctx.current.take().unwrap_or(Operator::SingleRow);
                    let alias = self.next_alias("fmt");
                    // Build gremlin_format_by args: pattern, then each by-alias value
                    let mut args = vec![
                        Expr::Variable(ctx.format_pre_alias.clone()),
                        Expr::Str(ctx.format_pattern.clone().into_boxed_str()),
                    ];
                    for by_alias in &ctx.format_by_aliases {
                        args.push(Expr::Variable(by_alias.clone()));
                    }
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_format_by"),
                            args,
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    let project = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(project),
                    });
                    ctx.current_alias = alias;
                    ctx.format_by_aliases.clear();
                }
                ctx.pending_format = false;
                // Apply collected select by() modulations cyclically
                if ctx.pending_select && !ctx.select_by_mods.is_empty() {
                    self.apply_pending_select_mods(
                        &mut ctx.current,
                        &mut ctx.current_alias,
                        &ctx.select_by_mods,
                    )?;
                }
                ctx.pending_select = false;
            }
            // Flush pending choose output alias on the first non-Option step
            if !matches!(step, GremlinStep::Option(..)) {
                if let Some(alias) = ctx.pending_choose_output.take() {
                    ctx.current_alias = alias;
                }
            }
            match self.dispatch_step(step, &mut ctx)? {
                StepFlow::Continue => continue,
                StepFlow::Proceed => {}
            }

            // Generic path tracking: if the ctx.current alias changed and this is
            // a path-contributing step, record it for path() reconstruction.
            if self.needs_path_tracking && ctx.current_alias != pre_step_alias {
                let is_modulator = matches!(
                    step,
                    GremlinStep::By(..)
                        | GremlinStep::ByTraversal(..)
                        | GremlinStep::From(_)
                        | GremlinStep::To(_)
                        | GremlinStep::As(_)
                        | GremlinStep::Times(_)
                        | GremlinStep::Until(_)
                        | GremlinStep::Emit
                        | GremlinStep::EmitPred(_)
                        | GremlinStep::Option(..)
                        | GremlinStep::WithSideEffect(..)
                        | GremlinStep::WithSack(..)
                        | GremlinStep::WithBulkFalse
                        | GremlinStep::ProductiveByStrategy
                        | GremlinStep::ReadOnlyStrategy
                        | GremlinStep::SeedStrategy(_)
                        | GremlinStep::ReservedKeysVerificationStrategy(_)
                        | GremlinStep::EdgeLabelVerificationStrategy
                        | GremlinStep::WithoutConnectiveStrategy
                        | GremlinStep::Property(..)
                        | GremlinStep::Inject(_)
                );
                // Don't re-add aliases that are already explicitly tracked
                let already_tracked = ctx.path_aliases.last() == Some(&ctx.current_alias);
                if !is_modulator && !already_tracked {
                    ctx.path_aliases.push(ctx.current_alias.clone());
                    self.path_tracking.push(ctx.current_alias.clone());
                }
            }
        }

        // Flush deferred choose output alias
        if let Some(alias) = ctx.pending_choose_output.take() {
            ctx.current_alias = alias;
        }

        // Flush pending select by-mods (handles by(T.key) as last step)
        if ctx.pending_select && !ctx.select_by_mods.is_empty() {
            self.apply_pending_select_mods(
                &mut ctx.current,
                &mut ctx.current_alias,
                &ctx.select_by_mods,
            )?;
        }

        // Flush accumulated path by-keys into the GremlinPath/CyclicPath operator
        if ctx.pending_path && !ctx.path_by_keys.is_empty() {
            match &mut ctx.current {
                Some(Operator::GremlinPath {
                    ref mut by_keys, ..
                }) => {
                    *by_keys = ctx.path_by_keys;
                }
                Some(Operator::GremlinCyclicPath {
                    ref mut by_keys, ..
                }) => {
                    *by_keys = ctx.path_by_keys;
                }
                Some(Operator::GremlinSimplePath {
                    ref mut by_keys, ..
                }) => {
                    *by_keys = ctx.path_by_keys;
                }
                _ => {}
            }
        }

        // If Order was set but no By followed, apply default sort
        if ctx.pending_order && ctx.pending_order_local {
            // order(Scope.local) with no by() — default sort
            if let Some(child) = ctx.current.take() {
                let alias = self.next_alias("srtl");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_sort_local"),
                        args: vec![Expr::Variable(ctx.current_alias.clone())],
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
            }
        } else if ctx.pending_order {
            if let Some(child) = ctx.current.take() {
                ctx.current = Some(Operator::Sort {
                    items: vec![(Expr::Variable(ctx.current_alias.clone()), true)],
                    gremlin_order: true,
                    child: Box::new(child),
                });
            }
        }

        // If a withSack() was present, inject __sack initialization into the
        // bottom-most leaf operator of the tree.
        if let Some(init_expr) = ctx.pending_sack_init {
            if let Some(ref mut op) = ctx.current {
                inject_sack_init(op, &init_expr);
            }
        }

        // Validate: emit/until/times without repeat is an error
        if ctx.pending_emit || ctx.pending_until.is_some() || ctx.pending_times.is_some() {
            return Err(CompileError::InvalidSequence(
                "The repeat()-traversal was not defined".into(),
            ));
        }

        // Flush any remaining addE defaults (traversal ends with addE group)
        if let Some(ref traverser) = ctx.adde_traverser {
            if let Some(ref mut op) = ctx.current {
                rewrite_create_alias(op, "__src", traverser);
                rewrite_create_alias(op, "__dst", traverser);
            }
        }

        // Flush pending math without by() modulation
        if ctx.pending_math {
            let child = ctx.current.take().unwrap_or(Operator::SingleRow);
            let alias = self.next_alias("math");
            let mut args = vec![
                Expr::Variable(ctx.pre_math_alias.clone()),
                Expr::Str(ctx.pending_math_expr.clone().into_boxed_str()),
            ];
            for var_name in extract_math_variables(&ctx.pending_math_expr) {
                let se_key = format!("__side_effect_{}", var_name);
                if let Some(se_ident) = self.alias_map.get(&var_name) {
                    args.push(Expr::Str(var_name.into_boxed_str()));
                    args.push(Expr::Variable(se_ident.clone()));
                } else if self.side_effect_values.contains_key(&var_name) {
                    args.push(Expr::Str(var_name.into_boxed_str()));
                    args.push(Expr::Variable(Identifier::new(se_key)));
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
        }

        if ctx.pending_sample {
            let child = ctx.current.take().unwrap_or(Operator::SingleRow);
            ctx.current = Some(Operator::GremlinSample {
                count: ctx.pending_sample_count as usize,
                weight: ctx.pending_sample_weight,
                seed: self.seed,
                child: Box::new(child),
            });
        }

        // Finalize any remaining format with by() modulations
        if ctx.pending_format && !ctx.format_by_aliases.is_empty() {
            let child = ctx.current.take().unwrap_or(Operator::SingleRow);
            let alias = self.next_alias("fmt");
            let mut args = vec![
                Expr::Variable(ctx.format_pre_alias.clone()),
                Expr::Str(ctx.format_pattern.clone().into_boxed_str()),
            ];
            for by_alias in &ctx.format_by_aliases {
                args.push(Expr::Variable(by_alias.clone()));
            }
            let mut items = vec![(
                Expr::FunctionCall {
                    name: SmolStr::new("gremlin_format_by"),
                    args,
                    distinct: false,
                },
                Some(alias.clone()),
            )];
            items.extend(self.label_carry_items());
            let project = Operator::Project {
                items,
                child: Box::new(child),
            };
            ctx.current = Some(Operator::Filter {
                predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                child: Box::new(project),
            });
            ctx.current_alias = alias;
        }

        if !ctx.pending_where_by_targets.is_empty() && !ctx.pending_where_by_modulators.is_empty() {
            if let Some(Operator::Filter {
                ref mut predicate, ..
            }) = ctx.current
            {
                finalize_where_by_modulation(
                    predicate,
                    &ctx.pending_where_by_targets,
                    &ctx.pending_where_by_modulators,
                    self.productive_by,
                );
            }
        }

        self.last_alias = ctx.current_alias;
        ctx.current.ok_or(CompileError::MissingChild)
    }
}
