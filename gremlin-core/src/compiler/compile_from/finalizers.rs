//! Epilogue finalizers for `compile_from`.
//!
//! After the per-step loop completes, several pending modulator states may
//! still be unflushed (order/sample/format/math/select/path/where, plus
//! sack init injection and addE default rewrites). This module owns those
//! end-of-traversal finalizers as a single inherent helper.

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::super::compile_from_context::CompileFromContext;
use super::super::helpers::*;
use super::super::{CompileError, GremlinCompiler};

impl GremlinCompiler {
    /// Run all end-of-traversal flushes and return the final operator tree.
    ///
    /// Mirrors the original tail of `compile_from` (post-loop body).
    pub(super) fn finalize_compile_from(
        &mut self,
        mut ctx: CompileFromContext,
    ) -> Result<Operator, CompileError> {
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
