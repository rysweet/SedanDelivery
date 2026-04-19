//! `Operator::children` — direct child operator references for structural traversal.

use crate::planner::operators::{GremlinSelectTraversal, Operator};

impl Operator {
    /// Return references to all direct child operators.
    pub fn children(&self) -> Vec<&Operator> {
        match self {
            Operator::SingleRow
            | Operator::NodeScan { .. }
            | Operator::IndexScan { .. }
            | Operator::NodeByIdSeek { .. }
            | Operator::ShortestPath { .. } => vec![],

            Operator::Filter { child, .. }
            | Operator::Project { child, .. }
            | Operator::Aggregate { child, .. }
            | Operator::Distinct { child, .. }
            | Operator::Sort { child, .. }
            | Operator::Limit { child, .. }
            | Operator::Skip { child, .. }
            | Operator::Unwind { child, .. }
            | Operator::Set { child, .. }
            | Operator::Remove { child, .. }
            | Operator::Delete { child, .. }
            | Operator::ProduceResults { child, .. }
            | Operator::Expand { child, .. }
            | Operator::VarLenExpand { child, .. }
            | Operator::AssemblePath { child, .. }
            | Operator::MergeRelationship { child, .. }
            | Operator::GremlinSideEffect { child, .. }
            | Operator::Cap { child, .. }
            | Operator::GremlinAggregate { child, .. }
            | Operator::GremlinPath { child, .. }
            | Operator::GremlinTree { child, .. }
            | Operator::GremlinSample { child, .. }
            | Operator::GremlinMapSlice { child, .. }
            | Operator::GremlinSimplePath { child, .. }
            | Operator::GremlinCyclicPath { child, .. }
            | Operator::GremlinIndex { child, .. }
            | Operator::GremlinBarrier { child, .. } => vec![child],

            Operator::GremlinSelect {
                child,
                by_traversals,
                ..
            } => {
                let mut v = vec![child.as_ref()];
                for entry in by_traversals {
                    if let Some(GremlinSelectTraversal::Traversal { op, .. }) = entry {
                        v.push(op.as_ref());
                    }
                }
                v
            }

            Operator::GremlinGroup {
                child,
                key_traversal,
                value_traversal,
                ..
            } => {
                let mut v = vec![child.as_ref()];
                if let Some((op, _)) = key_traversal {
                    v.push(op.as_ref());
                }
                if let Some((op, _)) = value_traversal {
                    v.push(op.as_ref());
                }
                v
            }

            Operator::GremlinProject { entries, child, .. } => {
                let mut v = vec![child.as_ref()];
                for (_, op, _, _) in entries {
                    v.push(op);
                }
                v
            }

            Operator::ForEach { body, child, .. } | Operator::Repeat { body, child, .. } => {
                vec![child, body]
            }

            Operator::Apply { outer, inner }
            | Operator::LeftOuterApply {
                preserved: outer,
                optional_pattern: inner,
            } => vec![outer, inner],

            Operator::Union { left, right, .. } => vec![left, right],

            Operator::Create { child, .. }
            | Operator::Merge { child, .. }
            | Operator::Call { child, .. } => match child {
                Some(c) => vec![c],
                None => vec![],
            },

            Operator::Choose {
                predicate,
                if_true,
                if_false,
                child,
                ..
            } => {
                let mut v = vec![child.as_ref(), predicate.as_ref(), if_true.as_ref()];
                if let Some(f) = if_false {
                    v.push(f.as_ref());
                }
                v
            }

            Operator::ChooseMap {
                selector,
                options,
                child,
                ..
            } => {
                let mut v = vec![child.as_ref(), selector.as_ref()];
                for (_, branch, _) in options {
                    v.push(branch.as_ref());
                }
                v
            }

            Operator::Coalesce {
                alternatives,
                child,
            } => {
                let mut v = vec![child.as_ref()];
                v.extend(alternatives.iter());
                v
            }

            Operator::GremlinMatch {
                patterns, child, ..
            } => {
                let mut v = vec![child.as_ref()];
                v.extend(patterns.iter());
                v
            }
        }
    }
}
