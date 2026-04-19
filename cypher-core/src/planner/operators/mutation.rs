//! CREATE pattern specs and SET/REMOVE item descriptors used by mutation
//! operators (`Operator::Create`, `Operator::Merge`, `Operator::Set`,
//! `Operator::Remove`, `Operator::MergeRelationship`).

use crate::ast::{Direction, Expr, Identifier, Label, RelType};

/// Type alias for a node specification in a CREATE pattern:
/// `(alias, labels, property_pairs)`
pub type CreateNodeSpec = (Option<Identifier>, Vec<Label>, Vec<(String, Expr)>);

/// Type alias for an edge specification in a CREATE pattern:
/// `(alias, src_alias, dst_alias, rel_type, direction, property_pairs)`
pub type CreateEdgeSpec = (
    Option<Identifier>,
    Identifier,
    Identifier,
    RelType,
    Direction,
    Vec<(String, Expr)>,
);

#[derive(Debug, Clone)]
pub struct CreatePattern {
    pub nodes: Vec<CreateNodeSpec>,
    pub edges: Vec<CreateEdgeSpec>,
}

#[derive(Debug, Clone)]
pub enum SetItem {
    Property {
        alias: Identifier,
        key: String,
        value: Expr,
    },
    Labels {
        alias: Identifier,
        labels: Vec<Label>,
    },
    DynamicLabels {
        alias: Identifier,
        value: Expr,
    },
    Map {
        alias: Identifier,
        map: Expr,
        merge: bool,
    },
}

#[derive(Debug, Clone)]
pub enum RemoveItem {
    Property {
        alias: Identifier,
        key: String,
    },
    Labels {
        alias: Identifier,
        labels: Vec<Label>,
    },
}
