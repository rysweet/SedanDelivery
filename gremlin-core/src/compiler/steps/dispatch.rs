//! Per-step dispatch for `compile_from`.
//!
//! Thin top-level `match` that routes each `GremlinStep` variant to a
//! per-category submodule (`source`, `filter`, `traversal`, ...). The actual
//! per-arm compilation logic lives in those submodules; this file only
//! decides which category handles the step.
//!
//! Each `compile_<category>_step` returns the same `StepFlow` semantics as
//! the original monolithic match — `Continue` skips post-step bookkeeping in
//! the caller's `for` loop, `Proceed` runs it.

use super::super::compile_from_context::CompileFromContext;
use super::super::{CompileError, GremlinCompiler};
use super::StepFlow;
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    /// Compile a single `GremlinStep` against the running compilation
    /// context. Returns `StepFlow::Continue` if the caller should skip
    /// post-step bookkeeping (formerly bare `continue;` inside the
    /// caller's `for step in steps` loop), or `StepFlow::Proceed`
    /// otherwise.
    pub(in crate::compiler) fn dispatch_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::V(..)
            | GremlinStep::E(..)
            | GremlinStep::Constant(..)
            | GremlinStep::Inject(..) =>
                self.compile_source_step(step, ctx),
            GremlinStep::Out(..)
            | GremlinStep::In(..)
            | GremlinStep::Both(..)
            | GremlinStep::OutE(..)
            | GremlinStep::InE(..)
            | GremlinStep::BothE(..)
            | GremlinStep::InV
            | GremlinStep::OutV
            | GremlinStep::OtherV
            | GremlinStep::BothV
            | GremlinStep::ToE(..) =>
                self.compile_traversal_step(step, ctx),
            GremlinStep::HasLabel(..)
            | GremlinStep::Has(..)
            | GremlinStep::HasPredicate(..)
            | GremlinStep::HasId(..)
            | GremlinStep::HasIdPredicate(..)
            | GremlinStep::Where(..)
            | GremlinStep::Not(..)
            | GremlinStep::And(..)
            | GremlinStep::Or(..)
            | GremlinStep::Is(..)
            | GremlinStep::HasKey(..)
            | GremlinStep::HasValue(..)
            | GremlinStep::HasValuePredicate(..)
            | GremlinStep::SimplePath
            | GremlinStep::CyclicPath
            | GremlinStep::Coin(..)
            | GremlinStep::FilterTraversal(..)
            | GremlinStep::All(..)
            | GremlinStep::Any(..)
            | GremlinStep::NonePred(..)
            | GremlinStep::Eq(..)
            | GremlinStep::Neq(..)
            | GremlinStep::Gt(..)
            | GremlinStep::Gte(..)
            | GremlinStep::Lt(..)
            | GremlinStep::Lte(..)
            | GremlinStep::Within(..)
            | GremlinStep::Without(..) =>
                self.compile_filter_step(step, ctx),
            GremlinStep::Values(..)
            | GremlinStep::ValueMap(..)
            | GremlinStep::Properties(..)
            | GremlinStep::Id
            | GremlinStep::Label
            | GremlinStep::Project(..)
            | GremlinStep::Identity
            | GremlinStep::Element
            | GremlinStep::Index
            | GremlinStep::IndexWithMap
            | GremlinStep::ElementMap(..)
            | GremlinStep::Map(..)
            | GremlinStep::FlatMap(..)
            | GremlinStep::Tree(..) =>
                self.compile_projection_step(step, ctx),
            GremlinStep::AddV(..)
            | GremlinStep::AddVTraversal(..)
            | GremlinStep::AddE(..)
            | GremlinStep::MergeV(..)
            | GremlinStep::MergeE(..)
            | GremlinStep::Property(..)
            | GremlinStep::From(..)
            | GremlinStep::To(..)
            | GremlinStep::Drop =>
                self.compile_mutation_step(step, ctx),
            GremlinStep::Count
            | GremlinStep::Sum
            | GremlinStep::Min
            | GremlinStep::Max
            | GremlinStep::Mean
            | GremlinStep::SumLocal
            | GremlinStep::MinLocal
            | GremlinStep::MaxLocal
            | GremlinStep::MeanLocal
            | GremlinStep::Group(..)
            | GremlinStep::GroupCount(..)
            | GremlinStep::Fold
            | GremlinStep::FoldSeed(..)
            | GremlinStep::Unfold =>
                self.compile_aggregation_step(step, ctx),
            GremlinStep::Repeat(..)
            | GremlinStep::Times(..)
            | GremlinStep::Until(..)
            | GremlinStep::Emit
            | GremlinStep::EmitPred(..)
            | GremlinStep::Choose(..)
            | GremlinStep::Coalesce(..)
            | GremlinStep::Union(..)
            | GremlinStep::Match(..)
            | GremlinStep::Optional(..)
            | GremlinStep::Option(..) =>
                self.compile_branch_step(step, ctx),
            GremlinStep::Order
            | GremlinStep::ByTraversal(..) =>
                self.compile_order_step(step, ctx),
            GremlinStep::By(..) =>
                self.compile_by_step(step, ctx),
            GremlinStep::Limit(..)
            | GremlinStep::Range(..)
            | GremlinStep::LimitLocal(..)
            | GremlinStep::RangeLocal(..)
            | GremlinStep::SkipLocal(..)
            | GremlinStep::SortLocal
            | GremlinStep::Dedup(..)
            | GremlinStep::Tail(..)
            | GremlinStep::TailLocal(..)
            | GremlinStep::Skip(..)
            | GremlinStep::Sample(..)
            | GremlinStep::SampleLocal(..)
            | GremlinStep::DedupLocal(..) =>
                self.compile_limit_step(step, ctx),
            GremlinStep::As(..)
            | GremlinStep::Select(..)
            | GremlinStep::SideEffect(..)
            | GremlinStep::Aggregate(..)
            | GremlinStep::Cap(..)
            | GremlinStep::None
            | GremlinStep::Barrier
            | GremlinStep::BarrierNormSack
            | GremlinStep::Discard
            | GremlinStep::Loops(..)
            | GremlinStep::Sack
            | GremlinStep::SackOp(..)
            | GremlinStep::WithSack(..)
            | GremlinStep::WithBulkFalse
            | GremlinStep::WithSideEffect(..) =>
                self.compile_side_effect_step(step, ctx),
            GremlinStep::Path =>
                self.compile_path_step(step, ctx),
            GremlinStep::ToLower
            | GremlinStep::ToUpper
            | GremlinStep::Trim
            | GremlinStep::LTrim
            | GremlinStep::RTrim
            | GremlinStep::Length
            | GremlinStep::Reverse
            | GremlinStep::Replace(..)
            | GremlinStep::Split(..)
            | GremlinStep::Substring(..)
            | GremlinStep::SubstringLocal(..)
            | GremlinStep::Concat(..)
            | GremlinStep::Conjoin(..)
            | GremlinStep::Format(..)
            | GremlinStep::StringOpLocal(..)
            | GremlinStep::AsString
            | GremlinStep::AsStringLocal
            | GremlinStep::AsNumber
            | GremlinStep::AsNumberTyped(..)
            | GremlinStep::AsBool
            | GremlinStep::Math(..)
            | GremlinStep::TypeOf
            | GremlinStep::AsDate =>
                self.compile_strings_step(step, ctx),
            GremlinStep::Combine(..)
            | GremlinStep::Intersect(..)
            | GremlinStep::Difference(..)
            | GremlinStep::Disjunct(..)
            | GremlinStep::MergeList(..)
            | GremlinStep::Product(..) =>
                self.compile_set_ops_step(step, ctx),
            GremlinStep::ProductiveByStrategy
            | GremlinStep::SeedStrategy(..)
            | GremlinStep::ReadOnlyStrategy
            | GremlinStep::ReservedKeysVerificationStrategy(..)
            | GremlinStep::EdgeLabelVerificationStrategy
            | GremlinStep::WithoutConnectiveStrategy
            | GremlinStep::Subgraph(..)
            | GremlinStep::Call(..)
            | GremlinStep::ShortestPath
            | GremlinStep::PageRank
            | GremlinStep::ConnectedComponent
            | GremlinStep::PeerPressure
            | GremlinStep::Io(..)
            | GremlinStep::DateAdd(..)
            | GremlinStep::DateDiff(..) =>
                self.compile_misc_step(step, ctx),
        }
    }
}
