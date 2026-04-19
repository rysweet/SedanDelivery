//! `sideEffect()`, `aggregate()`, and `cap()` step compilation.

#![allow(unused_imports)]

use cypher_core::ast::Identifier;
use cypher_core::planner::Operator;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    pub(super) fn compile_side_effect_branch(
        &mut self,
        effect_steps: &[GremlinStep],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let mut sub = self.sub_compiler();
        let effect = sub.compile_seeded(effect_steps, &ctx.current_alias)?;
        self.absorb(&sub);
        ctx.current = Some(Operator::GremlinSideEffect {
            effect: Box::new(effect),
            child: Box::new(child),
        });
        Ok(())
    }

    pub(super) fn compile_aggregate(
        &mut self,
        key: &str,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let is_set = self.set_keys.contains(key);
        let is_assign = self
            .reductions
            .get(key)
            .is_some_and(|(op, _)| op == "assign");
        ctx.current = Some(Operator::GremlinAggregate {
            key: key.to_string(),
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
        self.aggregate_keys.insert(key.to_string());
        ctx.pending_aggregate = true;
        Ok(())
    }

    pub(super) fn compile_cap(
        &mut self,
        keys: &[String],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        // Collect reduction metadata from compiler state
        let mut reductions: Vec<(String, String, f64)> = Vec::new();
        for k in keys {
            if let Some((op, init)) = self.reductions.get(k) {
                reductions.push((k.clone(), op.clone(), *init));
            }
        }
        ctx.current = Some(Operator::Cap {
            keys: keys.to_vec(),
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
        Ok(())
    }
}
