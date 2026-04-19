//! `mergeV()` / `mergeE()` step compilation.

#![allow(unused_imports)]

use cypher_core::ast::{Direction, Identifier, Label, RelType};
use cypher_core::planner::Operator;

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::helpers::*;
use super::super::super::{CompileError, GremlinCompiler};
use crate::types::GremlinValue;

impl GremlinCompiler {
    pub(super) fn compile_merge_v(
        &mut self,
        search_map: &GremlinValue,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        if self.read_only {
            return Err(CompileError::ReadOnlyViolation);
        }
        ctx.pending_properties.clear();
        let alias = self.next_alias("mv");
        ctx.current_alias = alias.clone();

        let mut labels = Vec::new();
        let mut properties = Vec::new();
        if let GremlinValue::Map(ref map) = search_map {
            for (k, v) in map {
                if k == "t[label]" || k == "T.label" {
                    if let GremlinValue::String(l) = v {
                        labels.push(Label::new(l.as_str()));
                    }
                } else if k != "t[id]" && k != "T.id" {
                    properties.push((k.clone(), gremlin_value_to_expr(v)));
                }
            }
        }

        ctx.current = Some(Operator::Merge {
            node_alias: Some(alias),
            labels,
            properties,
            on_create: vec![],
            on_match: vec![],
            child: ctx.current.take().map(Box::new),
        });
        Ok(())
    }

    pub(super) fn compile_merge_e(
        &mut self,
        search_map: &GremlinValue,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        if self.read_only {
            return Err(CompileError::ReadOnlyViolation);
        }
        ctx.pending_properties.clear();
        ctx.adde_traverser = Some(ctx.current_alias.clone());
        let rel_alias = self.next_alias("me");
        ctx.current_alias = rel_alias.clone();

        let mut rel_type_str = String::new();
        let mut properties = Vec::new();
        if let GremlinValue::Map(ref map) = search_map {
            for (k, v) in map {
                if k == "t[label]" || k == "T.label" {
                    if let GremlinValue::String(l) = v {
                        rel_type_str = l.clone();
                    }
                } else if !k.starts_with('(')
                    && !k.starts_with("Direction.")
                    && k != "t[id]"
                    && k != "T.id"
                {
                    properties.push((k.clone(), gremlin_value_to_expr(v)));
                }
            }
        }

        let src = Identifier::new("__src");
        let dst = Identifier::new("__dst");
        let rel_type = if rel_type_str.is_empty() {
            RelType::new("__default")
        } else {
            RelType::new(rel_type_str.as_str())
        };
        ctx.current = Some(Operator::MergeRelationship {
            src_alias: src,
            dst_alias: dst,
            rel_alias: Some(rel_alias),
            rel_type,
            direction: Direction::Outgoing,
            properties,
            on_create: vec![],
            on_match: vec![],
            child: Box::new(ctx.current.take().unwrap_or(Operator::SingleRow)),
        });
        Ok(())
    }
}
