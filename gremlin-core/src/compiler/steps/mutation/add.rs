//! `addV()` / `addV(traversal)` / `addE()` step compilation.

#![allow(unused_imports)]

use cypher_core::ast::{Direction, Expr, Identifier, Label, RelType};
use cypher_core::planner::{CreateEdgeSpec, CreatePattern, Operator, SetItem};

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::helpers::*;
use super::super::super::{CompileError, GremlinCompiler};

impl GremlinCompiler {
    pub(super) fn compile_add_v(
        &mut self,
        label: &str,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        if self.read_only {
            return Err(CompileError::ReadOnlyViolation);
        }
        ctx.pending_properties.clear();
        let alias = self.next_alias("nv");
        ctx.current_alias = alias.clone();
        let pattern = CreatePattern {
            nodes: vec![(
                Some(alias),
                vec![Label::new(label)],
                vec![], // properties added later
            )],
            edges: vec![],
        };
        ctx.current = Some(Operator::Create {
            patterns: vec![pattern],
            child: ctx.current.take().map(Box::new),
        });
        Ok(())
    }

    pub(super) fn compile_add_v_traversal(
        &mut self,
        label_steps: &[crate::bytecode::GremlinStep],
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        if self.read_only {
            return Err(CompileError::ReadOnlyViolation);
        }
        ctx.pending_properties.clear();
        let input = ctx.current.take().unwrap_or(Operator::SingleRow);
        let pre_add_alias = ctx.current_alias.clone();
        let mut sub = self.sub_compiler();
        let label_op = sub.compile_seeded(label_steps, &pre_add_alias)?;
        let label_alias = sub.last_alias.clone();
        self.absorb(&sub);

        let alias = self.next_alias("nv");
        ctx.current_alias = alias.clone();
        let create = Operator::Create {
            patterns: vec![CreatePattern {
                nodes: vec![(Some(alias.clone()), vec![], vec![])],
                edges: vec![],
            }],
            child: Some(Box::new(Operator::Apply {
                outer: Box::new(input),
                inner: Box::new(label_op),
            })),
        };
        ctx.current = Some(Operator::Set {
            items: vec![SetItem::DynamicLabels {
                alias: alias.clone(),
                value: Expr::Variable(label_alias),
            }],
            child: Box::new(create),
        });
        Ok(())
    }

    pub(super) fn compile_add_e(
        &mut self,
        label: &str,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        if self.read_only {
            return Err(CompileError::ReadOnlyViolation);
        }
        ctx.pending_properties.clear();
        ctx.adde_traverser = Some(ctx.current_alias.clone());
        let rel_alias = self.next_alias("ne");
        ctx.current_alias = rel_alias.clone();
        // Placeholder: src/dst to be set by From/To steps
        let src = Identifier::new("__src");
        let dst = Identifier::new("__dst");
        let edge_spec: CreateEdgeSpec = (
            Some(rel_alias),
            src,
            dst,
            RelType::new(label),
            Direction::Outgoing,
            vec![],
        );
        let pattern = CreatePattern {
            nodes: vec![],
            edges: vec![edge_spec],
        };
        ctx.current = Some(Operator::Create {
            patterns: vec![pattern],
            child: ctx.current.take().map(Box::new),
        });
        Ok(())
    }
}
