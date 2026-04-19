//! `path` category step compilation arms extracted from `dispatch.rs`.
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
    pub(super) fn compile_path_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::Path => {
                let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
                ctx.current = Some(Operator::GremlinPath {
                    child: Box::new(child),
                    path_aliases: ctx.path_aliases.clone(),
                    by_keys: vec![],
                    productive: self.productive_by,
                    from_label: None,
                    to_label: None,
                });
                ctx.current_alias = Identifier::new("path");
                ctx.pending_path = true;
                ctx.path_by_keys.clear();
                // Path has consumed the aliases — stop carrying them
                // through subsequent Projects.
                self.path_tracking.clear();
            }

            _ => unreachable!(
                "compile_path_step received non-path variant: {:?}",
                step
            ),
        }
        #[allow(unreachable_code)]
        Ok(StepFlow::Proceed)
    }
}
