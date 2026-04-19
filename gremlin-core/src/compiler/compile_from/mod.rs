//! `GremlinCompiler::compile_from` — the central per-step compilation routine.
//!
//! Walks a `Vec<GremlinStep>` left-to-right and folds each step into an
//! `Operator` tree. The body of this routine is intentionally split across
//! sibling submodules to keep each file focused:
//!
//! * [`per_step_pendings`] — pre-step pending modulator flushes
//! * [`finalizers`] — end-of-traversal flushes & sack injection
//!
//! `mod.rs` itself owns the prologue (preprocessing + prescans), the main
//! step loop (dispatch + path tracking), and the call to the finalizer.

use cypher_core::ast::Identifier;
use cypher_core::planner::Operator;

use super::compile_from_context::CompileFromContext;
use super::helpers::*;
use super::steps::StepFlow;
use super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;

mod finalizers;
mod per_step_pendings;

impl GremlinCompiler {
    /// Core compilation: walks steps left-to-right, wrapping each into the
    /// operator tree. `initial_op` / `initial_alias` allow callers to seed
    /// the compilation with an existing operator and alias.
    pub(super) fn compile_from(
        &mut self,
        steps: &[GremlinStep],
        initial_op: Option<Operator>,
        initial_alias: Identifier,
        initial_path_aliases: Vec<Identifier>,
    ) -> Result<Operator, CompileError> {
        if steps.is_empty() {
            return Ok(initial_op.unwrap_or(Operator::SingleRow));
        }

        // Preprocess infix and()/or() with no arguments into proper branching
        // forms — unless ConnectiveStrategy was explicitly disabled.
        let has_without_connective = steps
            .iter()
            .any(|s| matches!(s, GremlinStep::WithoutConnectiveStrategy));
        let preprocessed = if has_without_connective {
            steps.to_vec()
        } else {
            preprocess_infix_connectives(steps)
        };
        let steps = &preprocessed;

        // Pre-scan: detect if any step uses path tracking (path, simplePath, cyclicPath).
        // This enables carrying path aliases through Project operations.
        if !self.needs_path_tracking {
            self.needs_path_tracking = steps_need_path_tracking(steps);
        }
        // Pre-scan: detect if any select() uses Pop modifiers.
        if !self.needs_label_stacks {
            self.needs_label_stacks = steps_need_label_stacks(steps);
        }
        // Pre-scan: collect aggregate() keys so that within()/without() predicates
        // that reference a key defined LATER in the same step sequence (e.g. inside
        // repeat() bodies) can resolve to __gremlin_agg_<key> at compile time.
        collect_aggregate_keys_prescan(steps, &mut self.aggregate_keys);

        let mut ctx =
            CompileFromContext::new(initial_op, initial_alias.clone(), initial_path_aliases);
        // Sync path_tracking from initial_path_aliases
        self.path_tracking = ctx.path_aliases.clone();

        for step in steps {
            // Save alias before step to detect changes for path tracking
            let pre_step_alias = ctx.current_alias.clone();
            // Clear numeric subtype on value-changing steps; Is/As/filter steps preserve it.
            if !matches!(
                step,
                GremlinStep::Is(_)
                    | GremlinStep::As(_)
                    | GremlinStep::By(..)
                    | GremlinStep::ByTraversal(..)
            ) {
                self.numeric_subtype = None;
            }
            // When we leave the addE/mergeE modifier group (from/to/property/option),
            // fill in any __src/__dst placeholders that were never overridden.
            if ctx.adde_traverser.is_some()
                && !matches!(
                    step,
                    GremlinStep::From(_)
                        | GremlinStep::To(_)
                        | GremlinStep::Property(..)
                        | GremlinStep::Option(..)
                )
            {
                if let Some(ref traverser) = ctx.adde_traverser.take() {
                    if let Some(ref mut op) = ctx.current {
                        rewrite_create_alias(op, "__src", traverser);
                        rewrite_create_alias(op, "__dst", traverser);
                    }
                }
            }
            // Reset aggregate by-consumed flag on any non-By step
            if !matches!(step, GremlinStep::By(..) | GremlinStep::ByTraversal(..)) {
                self.flush_non_by_pendings(step, &mut ctx)?;
            }
            // Flush pending choose output alias on the first non-Option step
            if !matches!(step, GremlinStep::Option(..)) {
                if let Some(alias) = ctx.pending_choose_output.take() {
                    ctx.current_alias = alias;
                }
            }
            match self.dispatch_step(step, &mut ctx)? {
                StepFlow::Continue => continue,
                StepFlow::Proceed => {}
            }

            // Generic path tracking: if the ctx.current alias changed and this is
            // a path-contributing step, record it for path() reconstruction.
            if self.needs_path_tracking && ctx.current_alias != pre_step_alias {
                let is_modulator = matches!(
                    step,
                    GremlinStep::By(..)
                        | GremlinStep::ByTraversal(..)
                        | GremlinStep::From(_)
                        | GremlinStep::To(_)
                        | GremlinStep::As(_)
                        | GremlinStep::Times(_)
                        | GremlinStep::Until(_)
                        | GremlinStep::Emit
                        | GremlinStep::EmitPred(_)
                        | GremlinStep::Option(..)
                        | GremlinStep::WithSideEffect(..)
                        | GremlinStep::WithSack(..)
                        | GremlinStep::WithBulkFalse
                        | GremlinStep::ProductiveByStrategy
                        | GremlinStep::ReadOnlyStrategy
                        | GremlinStep::SeedStrategy(_)
                        | GremlinStep::ReservedKeysVerificationStrategy(_)
                        | GremlinStep::EdgeLabelVerificationStrategy
                        | GremlinStep::WithoutConnectiveStrategy
                        | GremlinStep::Property(..)
                        | GremlinStep::Inject(_)
                );
                // Don't re-add aliases that are already explicitly tracked
                let already_tracked = ctx.path_aliases.last() == Some(&ctx.current_alias);
                if !is_modulator && !already_tracked {
                    ctx.path_aliases.push(ctx.current_alias.clone());
                    self.path_tracking.push(ctx.current_alias.clone());
                }
            }
        }

        self.finalize_compile_from(ctx)
    }
}
