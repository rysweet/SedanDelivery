//! Small leaf helpers used across validator submodules:
//! kind/op formatting and error constructors.

use crate::ast::BinaryOp;
use crate::error::{GqlError, ParseError, PlanError};

use super::VarKind;

pub(in crate::planner::validator) fn kind_name(k: VarKind) -> &'static str {
    match k {
        VarKind::Node => "Node",
        VarKind::Relationship => "Relationship",
        VarKind::Path => "Path",
        VarKind::Value => "Value",
        VarKind::Untyped => "Untyped",
    }
}

pub(in crate::planner::validator) fn op_name(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::And => "AND",
        BinaryOp::Or => "OR",
        BinaryOp::Xor => "XOR",
        BinaryOp::Add | BinaryOp::Concat => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::Pow => "^",
        BinaryOp::StartsWith => "STARTS WITH",
        BinaryOp::EndsWith => "ENDS WITH",
        BinaryOp::Contains => "CONTAINS",
        BinaryOp::RegexMatch => "=~",
        _ => "operator",
    }
}

pub(in crate::planner::validator) fn plan_error_semantic(msg: &str) -> GqlError {
    GqlError::Plan(PlanError::SemanticError {
        message: msg.to_string(),
    })
}

pub(in crate::planner::validator) fn syntax_error(msg: &str) -> GqlError {
    GqlError::Parse(ParseError::Syntax {
        offset: 0,
        expected: msg.to_string(),
        found: String::new(),
    })
}
