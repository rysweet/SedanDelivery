//! `property()` step compilation.

#![allow(unused_imports)]

use cypher_core::ast::Expr;
use cypher_core::planner::{Operator, SetItem};

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::helpers::*;
use super::super::super::{CompileError, GremlinCompiler};
use crate::types::GremlinValue;

impl GremlinCompiler {
    pub(super) fn compile_property(
        &mut self,
        key: &str,
        value: &GremlinValue,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        if self.read_only {
            return Err(CompileError::ReadOnlyViolation);
        }
        // Helper: extract the select key from "__.select("x")" patterns
        let extract_select_key = |s: &str| -> Option<String> {
            let trimmed = s.trim_start_matches("__.");
            trimmed
                .strip_prefix("select(")
                .and_then(|s| s.strip_suffix(')'))
                .map(|inner| inner.trim_matches('"').trim_matches('\'').to_string())
        };
        // Resolve property key: if __.select("a") and "a" is a string
        // side-effect, use the string value as the property key name.
        let resolved_key = if let Some(ref sel_key) = extract_select_key(key) {
            if let Some(GremlinValue::String(s)) = self.side_effect_values.get(sel_key.as_str()) {
                s.clone()
            } else {
                key.to_string()
            }
        } else {
            key.to_string()
        };
        if let Some(ref reserved) = self.reserved_keys {
            let default_reserved = ["id", "label"];
            if default_reserved.contains(&resolved_key.as_str()) || reserved.contains(&resolved_key)
            {
                return Err(CompileError::ReservedKeyViolation(resolved_key));
            }
        }
        ctx.pending_properties
            .push((resolved_key.clone(), value.clone()));
        let child = ctx.current.take().ok_or(CompileError::MissingChild)?;
        // Resolve property value: if __.select("a"), emit Variable ref
        let val_expr = if let GremlinValue::String(s) = value {
            if let Some(ref sel_key) = extract_select_key(s) {
                if let Some(alias) = self.alias_map.get(sel_key.as_str()) {
                    Expr::Variable(alias.clone())
                } else {
                    gremlin_value_to_expr(value)
                }
            } else {
                gremlin_value_to_expr(value)
            }
        } else {
            gremlin_value_to_expr(value)
        };
        ctx.current = Some(Operator::Set {
            items: vec![SetItem::Property {
                alias: ctx.current_alias.clone(),
                key: resolved_key,
                value: val_expr,
            }],
            child: Box::new(child),
        });
        Ok(())
    }
}
