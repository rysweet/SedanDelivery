//! `misc` category step compilation arms extracted from `dispatch.rs`.
//!
//! Pure mechanical extraction — no behavior changes. Each arm body is
//! verbatim from the corresponding case in the original monolithic match.

#![allow(unused_imports)]

use std::collections::HashMap;

use cypher_core::ast::{BinaryOp, Direction, Expr, Identifier, Label, RelType};
use cypher_core::planner::{
    AggFunction, CreateEdgeSpec, CreatePattern, GremlinPop, Operator, SetItem,
};
use smol_str::SmolStr;

use super::super::compile_from_context::CompileFromContext;
use super::super::helpers::*;
use super::super::{CompileError, GremlinCompiler, SelectByModulator};
use super::StepFlow;
use crate::bytecode::{GremlinStep, Predicate};
use crate::types::GremlinValue;

impl GremlinCompiler {
    pub(super) fn compile_misc_step(
        &mut self,
        step: &GremlinStep,
        _ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::ProductiveByStrategy => {
                self.productive_by = true;
            }

            GremlinStep::SeedStrategy(seed) => {
                self.seed = Some(*seed);
            }

            GremlinStep::ReadOnlyStrategy => {
                self.read_only = true;
            }

            GremlinStep::ReservedKeysVerificationStrategy(keys) => {
                self.reserved_keys = Some(keys.clone());
            }

            GremlinStep::EdgeLabelVerificationStrategy => {
                self.edge_label_verify = true;
            }

            GremlinStep::WithoutConnectiveStrategy => {
                // Handled during preprocessing; no compiler state needed.
            }

            GremlinStep::Subgraph(_) => {
                return Err(CompileError::UnsupportedStep("subgraph".into()));
            }
            GremlinStep::Call(_) => {
                return Err(CompileError::UnsupportedStep("call".into()));
            }
            GremlinStep::ShortestPath => {
                return Err(CompileError::UnsupportedStep("shortestPath".into()));
            }
            GremlinStep::PageRank => {
                return Err(CompileError::UnsupportedStep("pageRank".into()));
            }
            GremlinStep::ConnectedComponent => {
                return Err(CompileError::UnsupportedStep("connectedComponent".into()));
            }
            GremlinStep::PeerPressure => {
                return Err(CompileError::UnsupportedStep("peerPressure".into()));
            }
            GremlinStep::Io(_) => {
                return Err(CompileError::UnsupportedStep("io".into()));
            }
            GremlinStep::DateAdd(..) => {
                return Err(CompileError::UnsupportedStep("dateAdd".into()));
            }
            GremlinStep::DateDiff(_) => {
                return Err(CompileError::UnsupportedStep("dateDiff".into()));
            }
            _ => unreachable!("compile_misc_step received non-misc variant: {:?}", step),
        }
        #[allow(unreachable_code)]
        Ok(StepFlow::Proceed)
    }
}
