//! MERGE and MERGE RELATIONSHIP operator implementations.
//!
//! Split into focused submodules:
//! - [`properties`]   shared property evaluation + canonical-key helpers
//! - [`node`]         `MERGE (n {...})` node match-or-create path
//! - [`rel`]          `MERGE (a)-[r:R]->(b)` relationship path (incl. batched fast-path)

use crate::ast::{Direction, Expr};
use crate::executor::engine::Row;
use crate::storage::NodeId;

mod node;
mod properties;
mod rel;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct PendingMergeEdgeKey {
    pub(super) src: NodeId,
    pub(super) dst: NodeId,
    pub(super) rel_type: String,
    pub(super) props_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct PendingMergeNodeKey {
    pub(super) labels: Vec<String>,
    pub(super) props_key: String,
}

pub(super) enum PendingMergeNodeSlot {
    Ready(Vec<Row>),
    Create { row: Row, create_index: usize },
    MatchedPending { row: Row, create_index: usize },
}

pub(super) enum PendingMergeEdgeSlot {
    Ready(Vec<Row>),
    Pending { row: Row, create_index: usize },
}

pub(super) struct MergeRelBatchSpec<'a> {
    pub(super) src_alias: &'a crate::ast::Identifier,
    pub(super) dst_alias: &'a crate::ast::Identifier,
    pub(super) rel_alias: Option<&'a crate::ast::Identifier>,
    pub(super) rel_type: &'a crate::ast::RelType,
    pub(super) direction: Direction,
    pub(super) properties: &'a [(String, Expr)],
}
