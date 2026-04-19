//! Per-step pending-state flushes for `compile_from`.
//!
//! When the walker encounters any non-`by()` step, a large block of
//! "pending modulator" state must be flushed (order/sample/format/math/
//! select/path/where/group/etc.). That block is extracted here as a
//! single inherent helper on `GremlinCompiler` to keep `mod.rs` focused
//! on the main step loop.

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::super::compile_from_context::CompileFromContext;
use super::super::helpers::*;
use super::super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    /// Flush all pending modulator state when a non-`by()` step arrives.
    ///
    /// Mirrors the original inline block from `compile_from`'s loop body.
    pub(super) fn flush_non_by_pendings(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
        Ok(())
    }
}
