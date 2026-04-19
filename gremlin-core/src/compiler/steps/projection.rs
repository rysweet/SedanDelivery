//! `projection` category step compilation arms extracted from `dispatch.rs`.
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
    pub(super) fn compile_projection_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::Values(keys) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let carry = self.label_carry_items();
                if keys.len() == 1 {
                    // Single key: project to a named alias, filter out nulls
                    // (Gremlin values() drops traversers without the property)
                    let alias = self.next_alias("val");
                    let mut items = vec![(
                        Expr::Property(
                            Box::new(Expr::Variable(ctx.current_alias.clone())),
                            SmolStr::new(&keys[0]),
                        ),
                        Some(alias.clone()),
                    )];
                    items.extend(carry);
                    let project = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(project),
                    });
                    ctx.current_alias = alias;
                } else if keys.is_empty() {
                    // values() with no keys: unwind all property values
                    let alias = self.next_alias("val");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_values_all"),
                            args: vec![Expr::Variable(ctx.current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(carry);
                    let project = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    // Unwind the list of all values
                    let uf_alias = self.next_alias("uf");
                    ctx.current = Some(Operator::Unwind {
                        list_expr: Expr::Variable(alias),
                        alias: uf_alias.clone(),
                        child: Box::new(project),
                    });
                    ctx.current_alias = uf_alias;
                } else {
                    // Multiple keys: collect values for each key into a list, then unwind
                    let list_alias = self.next_alias("vlist");
                    let mut fn_args = vec![Expr::Variable(ctx.current_alias.clone())];
                    for k in keys {
                        fn_args.push(Expr::Str(k.as_str().into()));
                    }
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_values_keys"),
                            args: fn_args,
                            distinct: false,
                        },
                        Some(list_alias.clone()),
                    )];
                    items.extend(carry);
                    let project = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    let uf_alias = self.next_alias("uf");
                    ctx.current = Some(Operator::Unwind {
                        list_expr: Expr::Variable(list_alias),
                        alias: uf_alias.clone(),
                        child: Box::new(project),
                    });
                    ctx.current_alias = uf_alias;
                }
            }

            GremlinStep::ValueMap(keys, token_mode) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("vmap");
                let mut args = vec![
                    Expr::Variable(ctx.current_alias.clone()),
                    Expr::Integer(i64::from(*token_mode)),
                ];
                for k in keys {
                    args.push(Expr::Str(k.as_str().into()));
                }
                let fc = Expr::FunctionCall {
                    name: SmolStr::new("gremlin_valuemap"),
                    args,
                    distinct: false,
                };
                let mut items = vec![(fc, Some(alias.clone()))];
                items.extend(self.label_carry_items());
                ctx.current_alias = alias;
                ctx.current = Some(Operator::Project {
                    items,
                    child: Box::new(child),
                });
                ctx.pending_valuemap = true;
                ctx.valuemap_by_consumed = false;
            }

            GremlinStep::Properties(keys) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let prop_alias = self.next_alias("prop");
                let carry = self.label_carry_items();
                if keys.is_empty() {
                    // properties() with no keys: unwind all properties as
                    // Map entries with ~key, ~value, ~element fields.
                    let list_alias = self.next_alias("plist");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_properties_entries"),
                            args: vec![Expr::Variable(ctx.current_alias.clone())],
                            distinct: false,
                        },
                        Some(list_alias.clone()),
                    )];
                    items.extend(carry);
                    let project = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    ctx.current = Some(Operator::Unwind {
                        list_expr: Expr::Variable(list_alias),
                        alias: prop_alias.clone(),
                        child: Box::new(project),
                    });
                } else if keys.len() == 1 {
                    // Single key: produce a property Map with ~key, ~value, ~element.
                    let list_alias = self.next_alias("plist");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_properties_entries"),
                            args: vec![
                                Expr::Variable(ctx.current_alias.clone()),
                                Expr::Str(keys[0].as_str().into()),
                            ],
                            distinct: false,
                        },
                        Some(list_alias.clone()),
                    )];
                    items.extend(carry);
                    let project = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    ctx.current = Some(Operator::Unwind {
                        list_expr: Expr::Variable(list_alias),
                        alias: prop_alias.clone(),
                        child: Box::new(project),
                    });
                } else {
                    // Multiple keys: build property entry list for each key and unwind
                    let list_alias = self.next_alias("plist");
                    let mut args = vec![Expr::Variable(ctx.current_alias.clone())];
                    for k in keys {
                        args.push(Expr::Str(k.as_str().into()));
                    }
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_properties_entries"),
                            args,
                            distinct: false,
                        },
                        Some(list_alias.clone()),
                    )];
                    items.extend(carry);
                    let project = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    ctx.current = Some(Operator::Unwind {
                        list_expr: Expr::Variable(list_alias),
                        alias: prop_alias.clone(),
                        child: Box::new(project),
                    });
                }
                ctx.current_alias = prop_alias;
            }

            GremlinStep::Id => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let id_alias = self.next_alias("id");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("id"),
                        args: vec![Expr::Variable(ctx.current_alias.clone())],
                        distinct: false,
                    },
                    Some(id_alias.clone()),
                )];
                items.extend(self.label_carry_items());
                ctx.current = Some(Operator::Project {
                    items,
                    child: Box::new(child),
                });
                ctx.current_alias = id_alias;
            }

            GremlinStep::Label => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let lbl_alias = self.next_alias("lbl");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_single_label"),
                        args: vec![Expr::Variable(ctx.current_alias.clone())],
                        distinct: false,
                    },
                    Some(lbl_alias.clone()),
                )];
                items.extend(self.label_carry_items());
                ctx.current = Some(Operator::Project {
                    items,
                    child: Box::new(child),
                });
                ctx.current_alias = lbl_alias;
            }

            GremlinStep::Project(keys) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                ctx.pre_project_alias = ctx.current_alias.clone();
                let mut items: Vec<(Expr, Option<Identifier>)> = keys
                    .iter()
                    .map(|k| {
                        (
                            Expr::Property(
                                Box::new(Expr::Variable(ctx.current_alias.clone())),
                                SmolStr::new(k),
                            ),
                            Some(Identifier::new(k.clone())),
                        )
                    })
                    .collect();
                items.extend(self.label_carry_items());
                ctx.current = Some(Operator::Project {
                    items,
                    child: Box::new(child),
                });
                ctx.pending_project = true;
                ctx.project_keys = keys.clone();
                ctx.project_by_count = 0;
            }

            GremlinStep::Identity => {
                // No-op: identity() passes through the ctx.current traverser unchanged.
                if ctx.current.is_none() {
                    return Err(CompileError::MissingChild);
                }
            }

            GremlinStep::Element => {
                // element() — extract ~element from property Map objects.
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let elem_alias = self.next_alias("elem");
                let mut items = vec![(
                    Expr::Property(
                        Box::new(Expr::Variable(ctx.current_alias.clone())),
                        SmolStr::new("~element"),
                    ),
                    Some(elem_alias.clone()),
                )];
                items.extend(self.label_carry_items());
                ctx.current = Some(Operator::Filter {
                    predicate: Expr::IsNotNull(Box::new(Expr::Variable(elem_alias.clone()))),
                    child: Box::new(Operator::Project {
                        items,
                        child: Box::new(child),
                    }),
                });
                ctx.current_alias = elem_alias;
            }

            GremlinStep::Index | GremlinStep::IndexWithMap => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let out_alias = self.next_alias("idx");
                let with_map = matches!(step, GremlinStep::IndexWithMap);
                ctx.current = Some(Operator::GremlinIndex {
                    child: Box::new(child),
                    alias: ctx.current_alias.clone(),
                    out_alias: out_alias.clone(),
                    with_map,
                });
                ctx.current_alias = out_alias;
            }

            GremlinStep::ElementMap(keys) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("emap");
                // elementMap as a function call that returns a Map value
                let mut args = vec![Expr::Variable(ctx.current_alias.clone())];
                for k in keys {
                    args.push(Expr::Str(k.as_str().into()));
                }
                let fc = Expr::FunctionCall {
                    name: SmolStr::new("gremlin_elementmap"),
                    args,
                    distinct: false,
                };
                let mut items = vec![(fc, Some(alias.clone()))];
                items.extend(self.label_carry_items());
                ctx.current_alias = alias;
                ctx.current = Some(Operator::Project {
                    items,
                    child: Box::new(child),
                });
            }

            GremlinStep::Map(map_steps) => {
                // map(traversal) — apply sub-traversal to each traverser
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let mut sub = self.sub_compiler();
                // Pass parent ctx.path_aliases so tree() inside local() can
                // access the full traversal path.
                sub.outer_path_aliases = ctx.path_aliases.clone();
                let map_op = sub.compile_seeded(map_steps, &ctx.current_alias)?;
                self.absorb(&sub);
                // Merge sub-compiler's path tracking into local ctx.path_aliases
                // so path()/simplePath() see aliases from the map body
                // (e.g., local(out().in().simplePath()).path()).
                for alias in &sub.path_tracking {
                    if !ctx.path_aliases.contains(alias) {
                        ctx.path_aliases.push(alias.clone());
                    }
                }
                ctx.current_alias = sub.last_alias.clone();
                ctx.current = Some(Operator::Apply {
                    outer: Box::new(child),
                    inner: Box::new(map_op),
                });
            }

            GremlinStep::FlatMap(fm_steps) => {
                // flatMap(traversal) — apply and flatten
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let mut sub = self.sub_compiler();
                let fm_op = sub.compile_seeded(fm_steps, &ctx.current_alias)?;
                self.absorb(&sub);
                ctx.current_alias = sub.last_alias.clone();
                ctx.current = Some(Operator::Apply {
                    outer: Box::new(child),
                    inner: Box::new(fm_op),
                });
            }

            GremlinStep::Tree(key) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                // Use outer path aliases when inside local()/map(),
                // otherwise use the ctx.current ctx.path_aliases.
                let tree_path_aliases = if !self.outer_path_aliases.is_empty() {
                    self.outer_path_aliases.clone()
                } else {
                    ctx.path_aliases.clone()
                };
                let alias = self.next_alias("tree");
                ctx.current = Some(Operator::GremlinTree {
                    child: Box::new(child),
                    path_aliases: tree_path_aliases,
                    side_effect_key: key.clone(),
                    alias: alias.clone(),
                });
                if let Some(k) = key {
                    self.aggregate_keys.insert(k.clone());
                } else {
                    // Barrier mode: tree becomes the new traverser value
                    ctx.current_alias = alias;
                }
            }
            _ => unreachable!(
                "compile_projection_step received non-projection variant: {:?}",
                step
            ),
        }
        #[allow(unreachable_code)]
        Ok(StepFlow::Proceed)
    }
}
