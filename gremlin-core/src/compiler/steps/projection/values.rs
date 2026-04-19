//! `values()` step compilation.

#![allow(unused_imports)]

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};

impl GremlinCompiler {
    pub(super) fn compile_values(
        &mut self,
        keys: &[String],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
        Ok(())
    }
}
