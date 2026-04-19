//! `properties()` step compilation.

#![allow(unused_imports)]

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};

impl GremlinCompiler {
    pub(super) fn compile_properties(
        &mut self,
        keys: &[String],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
        Ok(())
    }
}
