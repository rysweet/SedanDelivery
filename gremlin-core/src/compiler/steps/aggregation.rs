//! `aggregation` category step compilation arms extracted from `dispatch.rs`.
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
    pub(super) fn compile_aggregation_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::Count => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let count_alias = self.next_alias("cnt");
                ctx.current = Some(Operator::Aggregate {
                    keys: vec![],
                    aggregations: vec![(
                        AggFunction::Count,
                        Expr::Variable(ctx.current_alias.clone()),
                        None,
                        false,
                        count_alias.clone(),
                    )],
                    child: Box::new(child),
                });
                ctx.current_alias = count_alias;
            }

            GremlinStep::Sum => {
                // Use Fold + SumLocal for Gremlin semantics (empty → empty)
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let fold_alias = self.next_alias("fold");
                let folded = Operator::Aggregate {
                    keys: vec![],
                    aggregations: vec![(
                        AggFunction::Collect,
                        Expr::Variable(ctx.current_alias.clone()),
                        None,
                        false,
                        fold_alias.clone(),
                    )],
                    child: Box::new(child),
                };
                let sum_alias = self.next_alias("sum");
                let projected = Operator::Project {
                    items: vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_sum_local"),
                            args: vec![Expr::Variable(fold_alias)],
                            distinct: false,
                        },
                        Some(sum_alias.clone()),
                    )],
                    child: Box::new(folded),
                };
                // Filter out null (empty list produces null sum) unless
                // ProductiveByStrategy is active (all-null lists → null).
                if self.productive_by {
                    ctx.current = Some(projected);
                } else {
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(sum_alias.clone()))),
                        child: Box::new(projected),
                    });
                }
                ctx.current_alias = sum_alias;
            }

            GremlinStep::Min => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("min");
                let agg = Operator::Aggregate {
                    keys: vec![],
                    aggregations: vec![(
                        AggFunction::Min,
                        Expr::Variable(ctx.current_alias.clone()),
                        None,
                        false,
                        alias.clone(),
                    )],
                    child: Box::new(child),
                };
                if self.productive_by {
                    ctx.current = Some(agg);
                } else {
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(agg),
                    });
                }
                ctx.current_alias = alias;
            }

            GremlinStep::Max => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("max");
                let agg = Operator::Aggregate {
                    keys: vec![],
                    aggregations: vec![(
                        AggFunction::Max,
                        Expr::Variable(ctx.current_alias.clone()),
                        None,
                        false,
                        alias.clone(),
                    )],
                    child: Box::new(child),
                };
                if self.productive_by {
                    ctx.current = Some(agg);
                } else {
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(agg),
                    });
                }
                ctx.current_alias = alias;
            }

            GremlinStep::Mean => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("avg");
                let agg = Operator::Aggregate {
                    keys: vec![],
                    aggregations: vec![(
                        AggFunction::Avg,
                        Expr::Variable(ctx.current_alias.clone()),
                        None,
                        false,
                        alias.clone(),
                    )],
                    child: Box::new(child),
                };
                if self.productive_by {
                    ctx.current = Some(agg);
                } else {
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(agg),
                    });
                }
                ctx.current_alias = alias;
            }

            GremlinStep::SumLocal => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("suml");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_sum_local"),
                        args: vec![Expr::Variable(ctx.current_alias.clone())],
                        distinct: false,
                    },
                    Some(alias.clone()),
                )];
                items.extend(self.label_carry_items());
                let proj = Operator::Project {
                    items,
                    child: Box::new(child),
                };
                if self.productive_by {
                    ctx.current = Some(proj);
                } else {
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(proj),
                    });
                }
                ctx.current_alias = alias;
            }

            GremlinStep::MinLocal => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("minl");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_min_local"),
                        args: vec![Expr::Variable(ctx.current_alias.clone())],
                        distinct: false,
                    },
                    Some(alias.clone()),
                )];
                items.extend(self.label_carry_items());
                let proj = Operator::Project {
                    items,
                    child: Box::new(child),
                };
                if self.productive_by {
                    ctx.current = Some(proj);
                } else {
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(proj),
                    });
                }
                ctx.current_alias = alias;
            }

            GremlinStep::MaxLocal => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("maxl");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_max_local"),
                        args: vec![Expr::Variable(ctx.current_alias.clone())],
                        distinct: false,
                    },
                    Some(alias.clone()),
                )];
                items.extend(self.label_carry_items());
                let proj = Operator::Project {
                    items,
                    child: Box::new(child),
                };
                if self.productive_by {
                    ctx.current = Some(proj);
                } else {
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(proj),
                    });
                }
                ctx.current_alias = alias;
            }

            GremlinStep::MeanLocal => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("meanl");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_mean_local"),
                        args: vec![Expr::Variable(ctx.current_alias.clone())],
                        distinct: false,
                    },
                    Some(alias.clone()),
                )];
                items.extend(self.label_carry_items());
                let proj = Operator::Project {
                    items,
                    child: Box::new(child),
                };
                if self.productive_by {
                    ctx.current = Some(proj);
                } else {
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(proj),
                    });
                }
                ctx.current_alias = alias;
            }

            GremlinStep::Group(ref key) | GremlinStep::GroupCount(ref key) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("grp");
                let is_count = matches!(step, GremlinStep::GroupCount(..));
                let agg_fn = if is_count {
                    AggFunction::Count
                } else {
                    AggFunction::Collect
                };
                let has_side_effect = key.is_some();
                // Save the pre-group alias for by() modulation
                ctx.pre_group_alias = ctx.current_alias.clone();
                ctx.current = Some(Operator::GremlinGroup {
                    key_expr: Expr::Variable(ctx.current_alias.clone()),
                    value_expr: Expr::Variable(ctx.current_alias.clone()),
                    agg_fn,
                    alias: alias.clone(),
                    side_effect_key: key.clone(),
                    key_traversal: None,
                    value_traversal: None,
                    productive: self.productive_by,
                    key_is_identity: true,
                    explicit_fold: false,
                    child: Box::new(child),
                });
                // Named groups are side-effects (pass-through): keep ctx.current alias.
                // Unnamed groups consume the stream: switch to group alias.
                if !has_side_effect {
                    ctx.current_alias = alias;
                }
                ctx.pending_group = true;
                ctx.pending_group_by_count = 0;
                ctx.pending_group_is_count = is_count;
            }

            GremlinStep::Fold => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("fold");
                ctx.current = Some(Operator::Aggregate {
                    keys: vec![],
                    aggregations: vec![(
                        AggFunction::CollectAll,
                        Expr::Variable(ctx.current_alias.clone()),
                        None,
                        false,
                        alias.clone(),
                    )],
                    child: Box::new(child),
                });
                ctx.current_alias = alias;
            }

            GremlinStep::FoldSeed(seed, op_name) => {
                // fold(seed, Operator) — collect then reduce with a function.
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let collect_alias = self.next_alias("fold");
                let agg = Operator::Aggregate {
                    keys: vec![],
                    aggregations: vec![(
                        AggFunction::Collect,
                        Expr::Variable(ctx.current_alias.clone()),
                        None,
                        false,
                        collect_alias.clone(),
                    )],
                    child: Box::new(child),
                };
                let alias = self.next_alias("fred");
                let op_str = op_name
                    .strip_prefix("Operator.")
                    .unwrap_or(op_name)
                    .to_lowercase();
                let fc = Expr::FunctionCall {
                    name: SmolStr::new("gremlin_fold_reduce"),
                    args: vec![
                        Expr::Variable(collect_alias),
                        gremlin_value_to_expr(seed),
                        Expr::Str(op_str.into_boxed_str()),
                    ],
                    distinct: false,
                };
                let mut items = vec![(fc, Some(alias.clone()))];
                items.extend(self.label_carry_items());
                ctx.current = Some(Operator::Project {
                    items,
                    child: Box::new(agg),
                });
                ctx.current_alias = alias;
            }

            GremlinStep::Unfold => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("uf");
                ctx.current = Some(Operator::Unwind {
                    list_expr: Expr::Variable(ctx.current_alias.clone()),
                    alias: alias.clone(),
                    child: Box::new(child),
                });
                ctx.current_alias = alias;
                // After unfold, subsequent sub-traversals operate on
                // map entries — mark so select(Column.values) uses
                // the direct-extraction function.
                self.seed_is_map_entry = true;
            }

            _ => unreachable!(
                "compile_aggregation_step received non-aggregation variant: {:?}",
                step
            ),
        }
        #[allow(unreachable_code)]
        Ok(StepFlow::Proceed)
    }
}
