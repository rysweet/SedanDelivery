//! `repeat()` and its modulators (`times`, `until`, `emit`, `emit(pred)`).

#![allow(unused_imports)]

use cypher_core::ast::Identifier;
use cypher_core::planner::Operator;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    pub(super) fn compile_repeat(
        &mut self,
        label: &Option<String>,
        body_steps: &[GremlinStep],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        // Validate: inject() is not allowed inside repeat()
        if body_steps
            .iter()
            .any(|s| matches!(s, GremlinStep::Inject(_)))
        {
            return Err(CompileError::InvalidSequence(
                "The parent of inject()-step can not be repeat()-step".into(),
            ));
        }
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        let seed_alias = ctx.current_alias.clone();

        // Pre-scan: when Pop-select is downstream, check if the
        // body re-binds any existing label.  Create shadow aliases
        // so the executor can preserve original bindings for
        // Pop.first.
        let mut pop_origins: Vec<(String, String)> = Vec::new();
        if self.needs_label_stacks {
            for step in body_steps.iter() {
                if let GremlinStep::As(lbl) = step {
                    if let Some(orig) = self.alias_map.get(lbl).cloned() {
                        let shadow = self.next_alias("pop_orig");
                        self.label_stacks
                            .entry(lbl.clone())
                            .or_default()
                            .insert(0, shadow.clone());
                        pop_origins.push((shadow.as_str().to_string(), orig.as_str().to_string()));
                    }
                }
            }
        }

        let mut sub = self.sub_compiler();
        if self.seed_is_map_entry {
            sub.seed_is_map_entry = true;
        }
        let body = sub.compile_seeded(body_steps, &ctx.current_alias)?;
        let body_out_alias = sub.last_alias.clone();
        // Collect intermediate body path aliases (between seed and
        // body-output) so the executor can accumulate edge values
        // in __gremlin_path_acc for patterns like outE().inV().
        let body_pa: Vec<String> = if self.needs_path_tracking {
            sub.path_tracking
                .iter()
                .filter(|id| {
                    id.as_str() != seed_alias.as_str() && id.as_str() != body_out_alias.as_str()
                })
                .map(|id| id.as_str().to_string())
                .collect()
        } else {
            vec![]
        };
        self.absorb(&sub);
        // Apply any pending emit/until that appeared before the repeat
        let emit_val = ctx.pending_emit;
        ctx.pending_emit = false;
        let until_val = if let Some(until_steps) = ctx.pending_until.take() {
            let mut sub2 = self.sub_compiler();
            let until_op = sub2.compile_seeded(&until_steps, &ctx.current_alias)?;
            self.absorb(&sub2);
            Some(Box::new(until_op))
        } else {
            None
        };
        // If body produces a different alias, remap it back to the seed
        let remap = if body_out_alias.as_str() != seed_alias.as_str() {
            Some((
                body_out_alias.as_str().to_string(),
                seed_alias.as_str().to_string(),
            ))
        } else {
            None
        };
        let times_val = ctx.pending_times.take();
        let times_before_val = times_val.is_some();
        ctx.current = Some(Operator::Repeat {
            body: Box::new(body),
            times: times_val,
            until: until_val,
            emit: emit_val,
            emit_before: emit_val,
            emit_predicate: if let Some(ep_steps) = ctx.pending_emit_pred.take() {
                let mut sub3 = self.sub_compiler();
                let ep_op = sub3.compile_seeded(&ep_steps, &ctx.current_alias)?;
                self.absorb(&sub3);
                Some(Box::new(ep_op))
            } else {
                None
            },
            remap,
            times_before: times_before_val,
            loop_label: label.as_deref().map(Box::from),
            body_path_aliases: body_pa,
            pop_origins,
            needs_path: self.needs_path_tracking,
            child: Box::new(child),
        });
        Ok(())
    }

    pub(super) fn compile_times(&mut self, n: i64, ctx: &mut CompileFromContext) {
        // Modify preceding Repeat
        if let Some(Operator::Repeat {
            ref mut times,
            ref mut times_before,
            ..
        }) = ctx.current
        {
            *times = Some(n as usize);
            *times_before = false; // times() after repeat() → do-while
        } else {
            // times() before repeat() — store as pending
            ctx.pending_times = Some(n as usize);
        }
    }

    pub(super) fn compile_until(
        &mut self,
        until_steps: &[GremlinStep],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        if let Some(Operator::Repeat { ref mut until, .. }) = ctx.current {
            // Modify preceding Repeat
            let mut sub = self.sub_compiler();
            let until_op = sub.compile_seeded(until_steps, &ctx.current_alias)?;
            self.absorb(&sub);
            *until = Some(Box::new(until_op));
        } else {
            // until() before repeat() — store as pending
            ctx.pending_until = Some(until_steps.to_vec());
        }
        Ok(())
    }

    pub(super) fn compile_emit(&mut self, ctx: &mut CompileFromContext) {
        if let Some(Operator::Repeat {
            ref mut emit,
            ref mut emit_before,
            ..
        }) = ctx.current
        {
            *emit = true;
            // emit() after repeat() → post-emit (don't include starting traversers)
            *emit_before = false;
        } else {
            // emit() before repeat() — store as pending
            ctx.pending_emit = true;
        }
    }

    pub(super) fn compile_emit_pred(
        &mut self,
        pred_steps: &[GremlinStep],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        if let Some(Operator::Repeat {
            ref mut emit,
            ref mut emit_before,
            ref mut emit_predicate,
            ..
        }) = ctx.current
        {
            *emit = true;
            *emit_before = false;
            let mut sub = self.sub_compiler();
            let ep_op = sub.compile_seeded(pred_steps, &ctx.current_alias)?;
            self.absorb(&sub);
            *emit_predicate = Some(Box::new(ep_op));
        } else {
            // emit(pred) before repeat() — store as pending
            ctx.pending_emit = true;
            ctx.pending_emit_pred = Some(pred_steps.to_vec());
        }
        Ok(())
    }
}
