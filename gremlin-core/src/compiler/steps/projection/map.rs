//! `map()`, `flatMap()`, and `tree()` step compilation.

#![allow(unused_imports)]

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::Operator;
use smol_str::SmolStr;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    pub(super) fn compile_map(
        &mut self,
        map_steps: &[GremlinStep],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
        Ok(())
    }

    pub(super) fn compile_flat_map(
        &mut self,
        fm_steps: &[GremlinStep],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
        Ok(())
    }

    pub(super) fn compile_tree(
        &mut self,
        key: &Option<String>,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
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
        Ok(())
    }
}
