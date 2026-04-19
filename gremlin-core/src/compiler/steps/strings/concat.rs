//! String composition steps: `Concat`, `Conjoin`, `Format`, `StringOpLocal`.

use cypher_core::ast::Expr;
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::helpers::gremlin_value_to_expr;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    pub(super) fn compile_str_concat(
        &mut self,
        traversals: &[Vec<GremlinStep>],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
        Ok(())
    }

    pub(super) fn compile_str_conjoin(
        &mut self,
        sep: &str,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let alias = self.next_alias("join");
        let mut items = vec![(
            Expr::FunctionCall {
                name: SmolStr::new("gremlin_conjoin"),
                args: vec![
                    Expr::Variable(ctx.current_alias.clone()),
                    Expr::Str(sep.to_string().into_boxed_str()),
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
        Ok(())
    }

    pub(super) fn compile_str_format(
        &mut self,
        pattern: &str,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        if pattern.contains("%{_}") {
            // Pattern has positional placeholders — defer until by() modulations collected
            let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
            ctx.pending_format = true;
            ctx.format_pattern = pattern.to_string();
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
                .map(|(label, ident)| (SmolStr::new(label), Expr::Variable(ident.clone())))
                .collect();
            let mut args = vec![
                Expr::Variable(ctx.current_alias.clone()),
                Expr::Str(pattern.to_string().into_boxed_str()),
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
        Ok(())
    }

    pub(super) fn compile_str_op_local(
        &mut self,
        fn_name: &str,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
        Ok(())
    }
}
