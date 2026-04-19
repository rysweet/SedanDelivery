//! `filter` category step compilation arms extracted from `dispatch.rs`.
//!
//! The original monolithic `filter.rs` was decomposed into per-variant
//! submodules. This module re-assembles the public dispatch entry point
//! `compile_filter_step` by routing each `GremlinStep` filter variant to
//! the corresponding helper method defined in a submodule.
//!
//! All submodule helpers are pure mechanical extractions of the original
//! match-arm bodies — no behavior changes.

mod connectives;
mod filter_traversal;
mod has;
mod has_kv;
mod path_coin;
mod quantifiers;
mod where_not;

use super::super::compile_from_context::CompileFromContext;
use super::super::{CompileError, GremlinCompiler};
use super::StepFlow;
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    pub(super) fn compile_filter_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::HasLabel(labels) => self.compile_has_label(labels, ctx)?,
            GremlinStep::Has(key, value) => self.compile_has(key, value.as_ref(), ctx)?,
            GremlinStep::HasPredicate(key, pred) => self.compile_has_predicate(key, pred, ctx)?,
            GremlinStep::HasId(ids) => self.compile_has_id(ids, ctx)?,
            GremlinStep::HasIdPredicate(pred) => self.compile_has_id_predicate(pred, ctx)?,

            GremlinStep::Where(inner_steps) | GremlinStep::Not(inner_steps) => {
                let is_not = matches!(step, GremlinStep::Not(_));
                self.compile_where_or_not(inner_steps, is_not, ctx)?;
            }

            GremlinStep::And(branches) => self.compile_and(branches, ctx)?,
            GremlinStep::Or(branches) => self.compile_or(branches, ctx)?,
            GremlinStep::Is(predicate) => self.compile_is(predicate, ctx)?,

            GremlinStep::HasKey(keys) => self.compile_has_key(keys, ctx)?,
            GremlinStep::HasValue(values) => self.compile_has_value(values, ctx)?,
            GremlinStep::HasValuePredicate(pred) => self.compile_has_value_predicate(pred, ctx)?,

            GremlinStep::SimplePath => self.compile_simple_path(ctx)?,
            GremlinStep::CyclicPath => self.compile_cyclic_path(ctx)?,
            GremlinStep::Coin(probability) => self.compile_coin(*probability, ctx)?,

            GremlinStep::FilterTraversal(filter_steps) => {
                self.compile_filter_traversal(filter_steps, ctx)?
            }

            GremlinStep::All(pred) => self.compile_all(pred, ctx)?,
            GremlinStep::Any(pred) => self.compile_any(pred, ctx)?,
            GremlinStep::NonePred(pred) => self.compile_none_pred(pred, ctx)?,

            // Predicate steps used inside Where/Is — should not appear at top level
            GremlinStep::Eq(_)
            | GremlinStep::Neq(_)
            | GremlinStep::Gt(_)
            | GremlinStep::Gte(_)
            | GremlinStep::Lt(_)
            | GremlinStep::Lte(_)
            | GremlinStep::Within(_)
            | GremlinStep::Without(_) => {
                return Err(CompileError::InvalidSequence(
                    "predicate steps should not appear at the top level".into(),
                ));
            }

            // Unsupported steps — parsed successfully but not compilable
            _ => unreachable!(
                "compile_filter_step received non-filter variant: {:?}",
                step
            ),
        }
        Ok(StepFlow::Proceed)
    }
}
