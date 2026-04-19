//! `choose()` (predicate and value-based) and `option()` token branches.

#![allow(unused_imports)]

use cypher_core::ast::Identifier;
use cypher_core::planner::Operator;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::helpers::*;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;
use crate::types::GremlinValue;

impl GremlinCompiler {
    pub(super) fn compile_choose(
        &mut self,
        pred_steps: &[GremlinStep],
        true_steps: &[GremlinStep],
        false_steps: Option<&[GremlinStep]>,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let mut sub = self.sub_compiler();
        if true_steps.is_empty() && false_steps.is_none() {
            // 1-arg choose: value-based dispatch, options come from Option steps
            let selector = sub.compile_seeded(pred_steps, &ctx.current_alias)?;
            let sel_alias = sub.last_alias.clone();
            self.absorb(&sub);
            let choose_alias = self.next_alias("chm");
            ctx.current = Some(Operator::ChooseMap {
                selector: Box::new(selector),
                selector_alias: sel_alias,
                options: Vec::new(),
                child: Box::new(child),
                output_alias: Some(choose_alias.clone()),
            });
            // Defer alias switch until after all option() steps
            ctx.pending_choose_output = Some(choose_alias);
        } else {
            let pred_op = sub.compile_seeded(pred_steps, &ctx.current_alias)?;
            let true_op = sub.compile_seeded(true_steps, &ctx.current_alias)?;
            let t_alias = sub.last_alias.clone();
            let false_op = match false_steps {
                Some(steps) => Some(Box::new(sub.compile_seeded(steps, &ctx.current_alias)?)),
                None => None,
            };
            // For false branch: use the sub-compiler's alias if a false
            // branch exists, otherwise use the seed alias (ctx.current_alias
            // before choose) for pass-through rows.
            let f_alias = if false_steps.is_some() {
                sub.last_alias.clone()
            } else {
                ctx.current_alias.clone()
            };
            self.absorb(&sub);

            // Create a unified output alias for choose branches.
            let choose_alias = self.next_alias("ch");
            ctx.current_alias = choose_alias.clone();

            ctx.current = Some(Operator::Choose {
                predicate: Box::new(pred_op),
                if_true: Box::new(true_op),
                if_false: false_op,
                child: Box::new(child),
                true_alias: Some(t_alias),
                false_alias: Some(f_alias),
                output_alias: Some(choose_alias.clone()),
            });
        }
        Ok(())
    }

    pub(super) fn compile_option(
        &mut self,
        token: &GremlinValue,
        branch_steps: &[GremlinStep],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // Handle Merge.* tokens for mergeV/mergeE options
        let is_merge_option = matches!(
            token,
            GremlinValue::String(s) if s.starts_with("Merge.")
        );
        if is_merge_option {
            // mergeV/mergeE options — no-op for now
        } else if let Some(Operator::ChooseMap {
            ref mut options, ..
        }) = ctx.current
        {
            let mut sub = self.sub_compiler();
            let branch_op = sub.compile_seeded(branch_steps, &ctx.current_alias)?;
            let branch_alias = sub.last_alias.clone();
            self.absorb(&sub);
            let match_val = match token {
                GremlinValue::String(s) if s == "none" => None,
                _ => Some(gremlin_value_to_value(token)),
            };
            options.push((match_val, Box::new(branch_op), Some(branch_alias)));
        } else if ctx.current.is_none() {
            return Err(CompileError::MissingChild);
        }
        // If ctx.current is not ChooseMap or Merge, option is ignored (no-op).
        Ok(())
    }
}
