//! `as()` and `select()` step compilation.

#![allow(unused_imports)]

use std::collections::HashMap;

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::{GremlinPop, Operator};
use smol_str::SmolStr;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::helpers::*;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::Pop;

impl GremlinCompiler {
    pub(super) fn compile_as(&mut self, alias: &str, ctx: &mut CompileFromContext) {
        self.alias_map
            .insert(alias.to_string(), ctx.current_alias.clone());
        self.label_stacks
            .entry(alias.to_string())
            .or_default()
            .push(ctx.current_alias.clone());
    }

    pub(super) fn compile_select(
        &mut self,
        pop: Option<Pop>,
        keys: &[String],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
                                            .map(|l| Expr::Str(l.to_string().into_boxed_str()))
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
                let fn_name = if matches!(&child, Operator::Unwind { .. }) || self.seed_is_map_entry
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
        Ok(())
    }
}
