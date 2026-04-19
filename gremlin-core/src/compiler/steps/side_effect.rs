//! `side_effect` category step compilation arms extracted from `dispatch.rs`.
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
    pub(super) fn compile_side_effect_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::As(alias) => {
                self.alias_map.insert(alias.clone(), ctx.current_alias.clone());
                self.label_stacks
                    .entry(alias.clone())
                    .or_default()
                    .push(ctx.current_alias.clone());
            }

            GremlinStep::Select(pop, keys) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;

                // Handle Column.keys / Column.values special selects
                if keys.len() == 1
                    && (keys[0] == "Column.keys"
                        || keys[0] == "Column.values"
                        || keys[0] == "keys"
                        || keys[0] == "values")
                {
                    // Special case: path().select(Column.keys) — return the
                    // path's step label sets as a List of Lists.  Each entry
                    // is the sorted set of user labels for that path step;
                    // steps with no labels are omitted (TinkerPop semantics).
                    let is_keys = keys[0] == "Column.keys" || keys[0] == "keys";

                    // Use Option so we can conditionally consume `child`
                    // without Rust thinking it's always moved.
                    let mut child_opt = Some(child);

                    let handled_as_path_keys = if is_keys {
                        if let Some(Operator::GremlinPath {
                            ref path_aliases, ..
                        }) = child_opt
                        {
                            // Invert alias_map: internal_alias → [user_label, ...]
                            let mut alias_to_labels: HashMap<&str, Vec<&str>> = HashMap::new();
                            for (label, alias) in &self.alias_map {
                                alias_to_labels
                                    .entry(alias.as_str())
                                    .or_default()
                                    .push(label.as_str());
                            }
                            let label_sets: Vec<Expr> = path_aliases
                                .iter()
                                .filter_map(|alias| {
                                    let mut lbls: Vec<&str> = alias_to_labels
                                        .get(alias.as_str())
                                        .cloned()
                                        .unwrap_or_default();
                                    if lbls.is_empty() {
                                        None
                                    } else {
                                        lbls.sort_unstable();
                                        Some(Expr::MapLiteral(vec![(
                                            SmolStr::new("~set"),
                                            Expr::List(
                                                lbls.iter()
                                                    .map(|l| {
                                                        Expr::Str(
                                                            l.to_string().into_boxed_str(),
                                                        )
                                                    })
                                                    .collect(),
                                            ),
                                        )]))
                                    }
                                })
                                .collect();
                            let alias = self.next_alias("path_keys");
                            ctx.current = Some(Operator::Project {
                                items: vec![(Expr::List(label_sets), Some(alias.clone()))],
                                child: Box::new(child_opt.take().unwrap()),
                            });
                            ctx.current_alias = alias;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };

                    if !handled_as_path_keys {
                        let child = child_opt.take().unwrap();
                        let fn_name = if matches!(&child, Operator::Unwind { .. })
                            || self.seed_is_map_entry
                        {
                            if keys[0] == "Column.keys" || keys[0] == "keys" {
                                "gremlin_unfold_map_entry_key"
                            } else {
                                "gremlin_unfold_map_entry_value"
                            }
                        } else if keys[0] == "Column.keys" || keys[0] == "keys" {
                            "gremlin_column_key"
                        } else {
                            "gremlin_select_values"
                        };
                        let alias = self.next_alias(fn_name);
                        let mut items = vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new(fn_name),
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
                    } // end if !handled_as_path_keys
                } else {
                    let select_keys: Vec<(Identifier, String)> = keys
                        .iter()
                        .map(|k| {
                            let var = if let Some(label) = extract_select_traversal_label(k) {
                                let key_alias = self
                                    .alias_map
                                    .get(&label)
                                    .cloned()
                                    .unwrap_or_else(|| Identifier::new(label));
                                Identifier::new(format!(
                                    "__map_select__{}__keyalias__{}",
                                    ctx.current_alias.0, key_alias.0
                                ))
                            } else {
                                self.alias_map
                                    .get(k)
                                    .cloned()
                                    .unwrap_or_else(|| Identifier::new(k.clone()))
                            };
                            (var, k.clone())
                        })
                        .collect();
                    // Build label stacks for Pop support
                    let stacks: Vec<(String, Vec<Identifier>)> = keys
                        .iter()
                        .map(|k| {
                            let stack = self.label_stacks.get(k).cloned().unwrap_or_default();
                            (k.clone(), stack)
                        })
                        .collect();
                    // Map bytecode Pop to planner GremlinPop
                    let planner_pop = pop.map(|p| match p {
                        crate::bytecode::Pop::First => GremlinPop::First,
                        crate::bytecode::Pop::Last => GremlinPop::Last,
                        crate::bytecode::Pop::All => GremlinPop::All,
                        crate::bytecode::Pop::Mixed => GremlinPop::Mixed,
                    });
                    // Update ctx.current_alias if single key select
                    if keys.len() == 1 {
                        ctx.current_alias = select_keys[0].0.clone();
                    } else {
                        // Multi-key: set alias to the Map output column name
                        ctx.current_alias = Identifier::new(keys.join("_"));
                    }
                    ctx.current = Some(Operator::GremlinSelect {
                        keys: select_keys,
                        by_traversals: Vec::new(),
                        productive: self.productive_by,
                        pop: planner_pop,
                        label_stacks: stacks,
                        child: Box::new(child),
                    });
                    ctx.pending_select = true;
                    ctx.select_by_mods.clear();
                }
            }

            GremlinStep::SideEffect(effect_steps) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let mut sub = self.sub_compiler();
                let effect = sub.compile_seeded(effect_steps, &ctx.current_alias)?;
                self.absorb(&sub);
                ctx.current = Some(Operator::GremlinSideEffect {
                    effect: Box::new(effect),
                    child: Box::new(child),
                });
            }

            GremlinStep::Aggregate(key) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let is_set = self.set_keys.contains(key);
                let is_assign = self
                    .reductions
                    .get(key)
                    .is_some_and(|(op, _)| op == "assign");
                ctx.current = Some(Operator::GremlinAggregate {
                    key: key.clone(),
                    child: Box::new(child),
                    value_alias: ctx.current_alias.clone(),
                    by_keys: vec![],
                    by_traversal: None,
                    productive: self.productive_by,
                    set_semantics: is_set,
                    assign_semantics: is_assign,
                });
                // Track aggregate keys separately for within/without
                // resolution (not in alias_map to avoid conflicts).
                self.aggregate_keys.insert(key.clone());
                ctx.pending_aggregate = true;
            }

            GremlinStep::Cap(keys) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                // Collect reduction metadata from compiler state
                let mut reductions: Vec<(String, String, f64)> = Vec::new();
                for k in keys {
                    if let Some((op, init)) = self.reductions.get(k) {
                        reductions.push((k.clone(), op.clone(), *init));
                    }
                }
                ctx.current = Some(Operator::Cap {
                    keys: keys.clone(),
                    child: Box::new(child),
                    reductions,
                });
                // After cap(), the result row uses cap key names as columns.
                // Update ctx.current_alias so subsequent steps reference the right column.
                if keys.len() == 1 {
                    ctx.current_alias = Identifier::new(keys[0].clone());
                } else {
                    // Multi-key cap: the executor creates a combined Map
                    // under a joined alias so unfold() can iterate entries.
                    ctx.current_alias = Identifier::new(keys.join("_"));
                }
            }

            GremlinStep::None => {
                // none() — produces no results (empty)
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                ctx.current = Some(Operator::Filter {
                    predicate: Expr::Bool(false),
                    child: Box::new(child),
                });
            }

            GremlinStep::Barrier => {
                // barrier() without sack merge: synchronization no-op.
                // Only generate GremlinBarrier when sack merge is active.
                if ctx.pending_sack_merge_op.is_some() {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let ca = ctx.current_alias.clone();
                    let out = self.next_alias("barrier");
                    ctx.current = Some(Operator::GremlinBarrier {
                        child: Box::new(child),
                        current_alias: ca.clone(),
                        out_alias: out.clone(),
                        norm_sack: false,
                        sack_merge_op: ctx.pending_sack_merge_op.clone(),
                        with_bulk: !ctx.with_bulk_false,
                    });
                    ctx.current_alias = out;
                } else if ctx.current.is_none() {
                    return Err(CompileError::MissingChild);
                }
            }

            GremlinStep::BarrierNormSack => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let ca = ctx.current_alias.clone();
                let out = self.next_alias("barrier");
                ctx.current = Some(Operator::GremlinBarrier {
                    child: Box::new(child),
                    current_alias: ca.clone(),
                    out_alias: out.clone(),
                    norm_sack: true,
                    sack_merge_op: ctx.pending_sack_merge_op.clone(),
                    with_bulk: !ctx.with_bulk_false,
                });
                ctx.current_alias = out;
            }

            GremlinStep::Discard => {
                // discard() — consume traversal, return empty
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                ctx.current = Some(Operator::Filter {
                    predicate: Expr::Bool(false),
                    child: Box::new(child),
                });
            }

            GremlinStep::Loops(ref label) => {
                // loops() or loops("label") — returns ctx.current loop iteration counter.
                // Inside a repeat body the executor injects __gremlin_loop_count
                // (unlabeled) or __gremlin_loop_count_<label> (labeled).
                let child = ctx.current.take().unwrap_or(Operator::SingleRow);
                let alias = self.next_alias("loops");
                let var_name = match label {
                    Some(l) => format!("__gremlin_loop_count_{l}"),
                    None => "__gremlin_loop_count".to_string(),
                };
                let loop_var = Identifier::new(&var_name);
                let mut items = vec![(Expr::Variable(loop_var), Some(alias.clone()))];
                items.extend(self.label_carry_items());
                ctx.current = Some(Operator::Project {
                    items,
                    child: Box::new(child),
                });
                ctx.current_alias = alias;
            }

            GremlinStep::Sack => {
                // sack() — return the sack value from the __sack column
                let child = ctx.current.take().unwrap_or(Operator::SingleRow);
                let alias = self.next_alias("sack");
                let sack_var = Identifier::new("__sack");
                let mut items = vec![(Expr::Variable(sack_var), Some(alias.clone()))];
                items.extend(self.label_carry_items());
                ctx.current = Some(Operator::Project {
                    items,
                    child: Box::new(child),
                });
                ctx.current_alias = alias;
            }

            GremlinStep::SackOp(op) => {
                // sack(operator) — apply: __sack = __sack OP current_value
                // Must preserve ctx.current_alias and all label columns.
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let sack_var = Identifier::new("__sack");
                let bin_op = match op.as_str() {
                    "sum" | "add" => BinaryOp::Add,
                    "minus" | "sub" => BinaryOp::Sub,
                    "mult" | "multiply" => BinaryOp::Mul,
                    "div" | "divide" => BinaryOp::Div,
                    "and" => BinaryOp::And,
                    "or" => BinaryOp::Or,
                    "assign" => {
                        let mut items = vec![
                            (Expr::Variable(ctx.current_alias.clone()), Some(sack_var)),
                            (
                                Expr::Variable(ctx.current_alias.clone()),
                                Some(ctx.current_alias.clone()),
                            ),
                        ];
                        items.extend(self.label_carry_items());
                        ctx.current = Some(Operator::Project {
                            items,
                            child: Box::new(child),
                        });
                        ctx.pending_sack_op = true;
                        return Ok(StepFlow::Continue);
                    }
                    _ => BinaryOp::Add,
                };
                let mut items = vec![
                    (
                        Expr::BinaryOp {
                            op: bin_op,
                            left: Box::new(Expr::Variable(sack_var.clone())),
                            right: Box::new(Expr::Variable(ctx.current_alias.clone())),
                        },
                        Some(sack_var),
                    ),
                    (
                        Expr::Variable(ctx.current_alias.clone()),
                        Some(ctx.current_alias.clone()),
                    ),
                ];
                items.extend(self.label_carry_items());
                ctx.current = Some(Operator::Project {
                    items,
                    child: Box::new(child),
                });
                ctx.pending_sack_op = true;
            }

            GremlinStep::WithSack(val, ref merge_op) => {
                ctx.pending_sack_init = Some(gremlin_value_to_expr(val));
                ctx.pending_sack_merge_op = merge_op.clone();
            }

            GremlinStep::WithBulkFalse => {
                ctx.with_bulk_false = true;
            }

            GremlinStep::WithSideEffect(key, val, ref reduce_op) => {
                // withSideEffect(key, value[, operator]) — register a named side-effect
                let se_alias = Identifier::new(format!("__side_effect_{}", key));
                // Collect carry items BEFORE inserting the new alias to avoid
                // a self-referencing carry item that would try to read the
                // column we're about to create.
                let carry = self.label_carry_items();
                self.alias_map.insert(key.clone(), se_alias.clone());
                // Store the literal value for compile-time key resolution
                self.side_effect_values.insert(key.clone(), val.clone());
                // Track set-semantics keys (initialized via `{...}` notation)
                if reduce_op.as_deref() == Some("set") {
                    self.set_keys.insert(key.clone());
                }
                // For reduce operators, store in compiler state for Cap to use.
                // Still inject the initial value as a column so downstream
                // select/where can resolve it (e.g., where(within('a'))).
                if let Some(op) = reduce_op {
                    if op == "assign" {
                        // Operator.assign: track via reductions so GremlinAggregate
                        // can detect it. No numeric init needed.
                        self.reductions.insert(key.clone(), (op.clone(), 0.0));
                    } else if op != "set" {
                        // "set" is tracked via set_keys, not reductions
                        let init_val = match val {
                            GremlinValue::Integer(n) => *n as f64,
                            GremlinValue::Float(f) => *f,
                            GremlinValue::Bool(b) if *b => 1.0,
                            _ => 0.0,
                        };
                        self.reductions.insert(key.clone(), (op.clone(), init_val));
                    }
                }
                // Always inject the side-effect value as a column so
                // downstream select/where/property can resolve it.
                let child = ctx.current.take().unwrap_or(Operator::SingleRow);
                let val_expr = gremlin_value_to_expr(val);
                let mut items = vec![(val_expr, Some(se_alias))];
                items.extend(carry);
                ctx.current = Some(Operator::Project {
                    items,
                    child: Box::new(child),
                });
            }

            _ => unreachable!(
                "compile_side_effect_step received non-side_effect variant: {:?}",
                step
            ),
        }
        #[allow(unreachable_code)]
        Ok(StepFlow::Proceed)
    }
}
