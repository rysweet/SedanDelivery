//! `strings` category step compilation arms extracted from `dispatch.rs`.
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
    pub(super) fn compile_strings_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::ToLower => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("str");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_toLower"),
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

            GremlinStep::ToUpper => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("str");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_toUpper"),
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

            GremlinStep::Trim => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("str");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_trim"),
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

            GremlinStep::LTrim => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("str");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_lTrim"),
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

            GremlinStep::RTrim => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("str");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_rTrim"),
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

            GremlinStep::Length => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("len");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_length"),
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

            GremlinStep::Reverse => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("rev");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_reverse"),
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

            GremlinStep::Replace(old, new) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("str");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_replace"),
                        args: vec![
                            Expr::Variable(ctx.current_alias.clone()),
                            Expr::Str(old.clone().into_boxed_str()),
                            Expr::Str(new.clone().into_boxed_str()),
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
            }

            GremlinStep::Split(sep) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("split");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_split"),
                        args: vec![
                            Expr::Variable(ctx.current_alias.clone()),
                            Expr::Str(sep.clone().into_boxed_str()),
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
            }

            GremlinStep::Substring(start, len) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("substr");
                let mut args =
                    vec![Expr::Variable(ctx.current_alias.clone()), Expr::Integer(*start)];
                if let Some(l) = len {
                    args.push(Expr::Integer(*l));
                }
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_substring"),
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

            GremlinStep::SubstringLocal(start, len) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("substrl");
                let mut args =
                    vec![Expr::Variable(ctx.current_alias.clone()), Expr::Integer(*start)];
                if let Some(l) = len {
                    args.push(Expr::Integer(*l));
                }
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_substring_local"),
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

            GremlinStep::Concat(traversals) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("concat");
                // Build concat args: ctx.current value + each sub-traversal result
                let mut args = vec![Expr::Variable(ctx.current_alias.clone())];
                let mut base = child;
                for trav in traversals {
                    if trav.is_empty() {
                        // Empty traversal → empty string
                        args.push(Expr::Str("".into()));
                        continue;
                    }
                    // Single constant → inline
                    if trav.len() == 1 {
                        if let GremlinStep::Constant(v) = &trav[0] {
                            args.push(gremlin_value_to_expr(v));
                            continue;
                        }
                    }
                    // Sub-traversal: compile and wrap in Apply.
                    // Limit inner to 1 result per outer row (Gremlin concat
                    // uses only the first value from inner traversals).
                    // Use compile_from with a seed so inject() adds to the
                    // incoming stream rather than replacing it.
                    let mut sub = self.sub_compiler();
                    let sub_op = if matches!(trav.first(), Some(GremlinStep::Inject(_))) {
                        sub.compile_from(
                            trav,
                            Some(Operator::SingleRow),
                            ctx.current_alias.clone(),
                            vec![ctx.current_alias.clone()],
                        )?
                    } else {
                        sub.compile_seeded(trav, &ctx.current_alias)?
                    };
                    let sub_alias = sub.last_alias.clone();
                    self.absorb(&sub);
                    let limited = Operator::Limit {
                        count: Expr::Integer(1),
                        child: Box::new(sub_op),
                    };
                    base = Operator::Apply {
                        outer: Box::new(base),
                        inner: Box::new(limited),
                    };
                    args.push(Expr::Variable(sub_alias));
                }
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_concat"),
                        args,
                        distinct: false,
                    },
                    Some(alias.clone()),
                )];
                items.extend(self.label_carry_items());
                ctx.current = Some(Operator::Project {
                    items,
                    child: Box::new(base),
                });
                ctx.current_alias = alias;
            }

            GremlinStep::Conjoin(sep) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("join");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_conjoin"),
                        args: vec![
                            Expr::Variable(ctx.current_alias.clone()),
                            Expr::Str(sep.clone().into_boxed_str()),
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
            }

            GremlinStep::Format(pattern) => {
                if pattern.contains("%{_}") {
                    // Pattern has positional placeholders — defer until by() modulations collected
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    ctx.pending_format = true;
                    ctx.format_pattern = pattern.clone();
                    ctx.format_pre_alias = ctx.current_alias.clone();
                    ctx.format_by_aliases.clear();
                    ctx.current = Some(child);
                } else {
                    let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("fmt");
                    // Build a map of as-label values for %{label} resolution
                    let label_pairs: Vec<(SmolStr, Expr)> = self
                        .alias_map
                        .iter()
                        .map(|(label, ident)| {
                            (SmolStr::new(label), Expr::Variable(ident.clone()))
                        })
                        .collect();
                    let mut args = vec![
                        Expr::Variable(ctx.current_alias.clone()),
                        Expr::Str(pattern.clone().into_boxed_str()),
                    ];
                    if !label_pairs.is_empty() {
                        args.push(Expr::MapLiteral(label_pairs));
                    }
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_format"),
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
                    // Filter out traversers where format couldn't resolve all placeholders
                    ctx.current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(project),
                    });
                    ctx.current_alias = alias;
                }
            }

            GremlinStep::StringOpLocal(fn_name) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("sloc");
                // Parse function name and optional extra args from \0-separated string
                let parts: Vec<&str> = fn_name.split('\0').collect();
                let func_name = parts[0];
                let mut args = vec![Expr::Variable(ctx.current_alias.clone())];
                for extra in &parts[1..] {
                    args.push(Expr::Str(extra.to_string().into_boxed_str()));
                }
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new(func_name),
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

            GremlinStep::AsString => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("str");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_asstring"),
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

            GremlinStep::AsStringLocal => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("strl");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_asstring_local"),
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

            GremlinStep::AsNumber => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("num");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_asnumber"),
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

            GremlinStep::AsNumberTyped(ref target_type) => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("num");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_asnumber_typed"),
                        args: vec![
                            Expr::Variable(ctx.current_alias.clone()),
                            Expr::Str(target_type.clone().into_boxed_str()),
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
                self.numeric_subtype = Some(target_type.clone());
            }

            GremlinStep::AsBool => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("bool");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_asBool"),
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

            GremlinStep::Math(expr_str) => {
                // Set ctx.pending_math so that a following by() can resolve variables
                ctx.pending_math = true;
                ctx.pending_math_expr = expr_str.clone();
                ctx.pre_math_alias = ctx.current_alias.clone();
            }

            GremlinStep::TypeOf => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("type");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_typeof"),
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

            // ── Collection steps ─────────────────────────────────────────────
            GremlinStep::AsDate => {
                // asDate() — convert epoch-millis integer to Date value.
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                let alias = self.next_alias("date");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_asdate"),
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
            _ => unreachable!(
                "compile_strings_step received non-strings variant: {:?}",
                step
            ),
        }
        #[allow(unreachable_code)]
        Ok(StepFlow::Proceed)
    }
}
