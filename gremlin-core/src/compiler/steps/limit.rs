//! `limit` category step compilation arms extracted from `dispatch.rs`.
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
    pub(super) fn compile_limit_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::Limit(n) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                ctx.current = Some(Operator::Limit {
                    count: Expr::Integer(*n),
                    child: Box::new(child),
                });
            }

            GremlinStep::Range(low, high) => {
                if *high < *low {
                    return Err(CompileError::InvalidSequence(format!(
                        "range({}, {}): high must be >= low",
                        low, high
                    )));
                }
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let skipped = if *low > 0 {
                    Operator::Skip {
                        count: Expr::Integer(*low),
                        child: Box::new(child),
                    }
                } else {
                    child
                };
                ctx.current = Some(Operator::Limit {
                    count: Expr::Integer(*high - *low),
                    child: Box::new(skipped),
                });
            }

            GremlinStep::LimitLocal(n) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                ctx.current = Some(Operator::GremlinMapSlice {
                    alias: ctx.current_alias.clone(),
                    lo: 0,
                    hi: *n as usize,
                    child: Box::new(child),
                });
            }

            GremlinStep::RangeLocal(low, high) => {
                // Flush pending order(Scope.local) so the sort is applied
                // before slicing.
                if ctx.pending_order && ctx.pending_order_local {
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
                    ctx.pending_order = false;
                    ctx.pending_order_local = false;
                }
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                ctx.current = Some(Operator::GremlinMapSlice {
                    alias: ctx.current_alias.clone(),
                    lo: *low as usize,
                    hi: *high as usize,
                    child: Box::new(child),
                });
            }

            GremlinStep::SkipLocal(n) => {
                // Flush pending order(Scope.local) so the sort is applied
                // before skipping.
                if ctx.pending_order && ctx.pending_order_local {
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
                    ctx.pending_order = false;
                    ctx.pending_order_local = false;
                }
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                ctx.current = Some(Operator::GremlinMapSlice {
                    alias: ctx.current_alias.clone(),
                    lo: *n as usize,
                    hi: usize::MAX,
                    child: Box::new(child),
                });
            }

            GremlinStep::SortLocal => {
                // Sort elements within a list/map value.
                // Defer to by() handler; if no by() follows, resolve at end.
                ctx.pending_order = true;
                ctx.pending_order_local = true;
            }

            GremlinStep::Dedup(keys) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                // dedup() — deduplicate by the ctx.current traverser value
                // dedup("a","b") — deduplicate by values at step labels
                let by_key = if keys.is_empty() {
                    Some(ctx.current_alias.to_string())
                } else if keys.len() == 1 {
                    // Resolve label to internal alias
                    let resolved = self
                        .alias_map
                        .get(&keys[0])
                        .map(|a| a.to_string())
                        .unwrap_or_else(|| keys[0].clone());
                    Some(resolved)
                } else {
                    // Multi-key dedup: resolve each label to its alias
                    let resolved: Vec<String> = keys
                        .iter()
                        .map(|k| {
                            self.alias_map
                                .get(k)
                                .map(|a| a.to_string())
                                .unwrap_or_else(|| k.clone())
                        })
                        .collect();
                    Some(resolved.join("\x1f"))
                };
                ctx.current = Some(Operator::Distinct {
                    child: Box::new(child),
                    by_key,
                    productive: self.productive_by,
                });
                ctx.pending_dedup = true;
            }

            GremlinStep::Tail(n) => {
                // tail(n): take last n rows (preserving existing order).
                // Uses a gremlin_tail marker so the executor skips early-
                // termination and takes from the end of the materialized
                // result set rather than the beginning.
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                ctx.current = Some(Operator::Limit {
                    count: Expr::FunctionCall {
                        name: SmolStr::new("gremlin_tail"),
                        args: vec![Expr::Integer(*n)],
                        distinct: false,
                    },
                    child: Box::new(child),
                });
            }

            GremlinStep::TailLocal(n) => {
                // Flush pending order(Scope.local) so the sort is applied
                // before taking the tail.
                if ctx.pending_order && ctx.pending_order_local {
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
                    ctx.pending_order = false;
                    ctx.pending_order_local = false;
                }
                // tail(Scope.local, n): take last n elements of list values
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("tail");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_tail_local"),
                        args: vec![Expr::Variable(ctx.current_alias.clone()), Expr::Integer(*n)],
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

            GremlinStep::Skip(n) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                ctx.current = Some(Operator::Skip {
                    count: Expr::Integer(*n),
                    child: Box::new(child),
                });
            }

            GremlinStep::Sample(n) => {
                // sample(n) is lowered when we know whether a by() modulator follows.
                ctx.pending_sample = true;
                ctx.sample_by_consumed = false;
                ctx.pending_sample_count = *n;
                ctx.pending_sample_weight = None;
            }

            GremlinStep::SampleLocal(n) => {
                // sample(Scope.local, n) samples within the ctx.current list/map value.
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                ctx.current = Some(Operator::Project {
                    items: vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_sample_local"),
                            args: vec![
                                Expr::Variable(ctx.current_alias.clone()),
                                Expr::Integer(*n),
                                self.seed.map_or(Expr::Null, Expr::Integer),
                            ],
                            distinct: false,
                        },
                        Some(ctx.current_alias.clone()),
                    )],
                    child: Box::new(child),
                });
            }

            GremlinStep::DedupLocal(keys) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("dedup_local");
                let mut args = vec![Expr::Variable(ctx.current_alias.clone())];
                for k in keys {
                    args.push(Expr::Str(k.clone().into_boxed_str()));
                }
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_dedup_local"),
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

            _ => unreachable!(
                "compile_limit_step received non-limit variant: {:?}",
                step
            ),
        }
        #[allow(unreachable_code)]
        Ok(StepFlow::Proceed)
    }
}
