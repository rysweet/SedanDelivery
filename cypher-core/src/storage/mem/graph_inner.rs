//! Per-graph mutable state for the in-memory backend, plus index/constraint
//! helpers that operate on a single `GraphInner`.

use indexmap::IndexMap;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::error::StorageError;
use crate::storage::{
    AdjEntry, ConstraintDescriptor, ConstraintType, EdgeData, IndexDescriptor, NodeData,
};
use crate::types::Value;

use super::indexable::{IndexableValue, PropIndex};

/// All mutable state for a single named graph.
#[derive(Clone)]
pub(crate) struct GraphInner {
    pub(crate) nodes: HashMap<u64, NodeData>,
    pub(crate) edges: HashMap<u64, EdgeData>,
    pub(crate) out_adj: HashMap<u64, Vec<AdjEntry>>,
    pub(crate) in_adj: HashMap<u64, Vec<AdjEntry>>,
    pub(crate) label_index: HashMap<String, HashSet<u64>>,
    pub(crate) reltype_index: HashMap<String, HashSet<u64>>,
    pub(crate) indexes: Vec<IndexDescriptor>,
    pub(crate) prop_index: PropIndex,
    pub(crate) constraints: HashSet<ConstraintDescriptor>,
    pub(crate) node_seq: u64,
    pub(crate) edge_seq: u64,
}

impl GraphInner {
    pub(crate) fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: HashMap::new(),
            out_adj: HashMap::new(),
            in_adj: HashMap::new(),
            label_index: HashMap::new(),
            reltype_index: HashMap::new(),
            indexes: Vec::new(),
            prop_index: BTreeMap::new(),
            constraints: HashSet::new(),
            node_seq: 0,
            edge_seq: 0,
        }
    }

    pub(crate) fn next_node_id(&mut self, graph_name: &str) -> Result<u64, StorageError> {
        // Shared counter: both nodes and edges draw from edge_seq so the
        // modern-graph gets vertex IDs 1-6 and edge IDs 7-12 (TinkerPop compat).
        self.edge_seq = self
            .edge_seq
            .checked_add(1)
            .ok_or_else(|| StorageError::IdOverflow {
                graph: graph_name.to_string(),
            })?;
        self.node_seq = self.edge_seq;
        Ok(self.edge_seq)
    }

    pub(crate) fn has_index(&self, label: &str, property: &str) -> bool {
        self.indexes
            .iter()
            .any(|i| i.label == label && i.property == property)
    }

    pub(crate) fn index_insert(
        &mut self,
        raw_id: u64,
        labels: &[String],
        props: &IndexMap<String, Value>,
    ) {
        for idx in &self.indexes {
            if labels.contains(&idx.label) {
                if let Some(val) = props.get(&idx.property) {
                    if let Some(iv) = IndexableValue::from_value(val) {
                        self.prop_index
                            .entry((idx.label.clone(), idx.property.clone(), iv))
                            .or_default()
                            .insert(raw_id);
                    }
                }
            }
        }
    }

    pub(crate) fn index_remove(
        &mut self,
        raw_id: u64,
        labels: &[String],
        props: &IndexMap<String, Value>,
    ) {
        for idx in &self.indexes {
            if labels.contains(&idx.label) {
                if let Some(val) = props.get(&idx.property) {
                    if let Some(iv) = IndexableValue::from_value(val) {
                        let key = (idx.label.clone(), idx.property.clone(), iv);
                        if let Some(set) = self.prop_index.get_mut(&key) {
                            set.remove(&raw_id);
                            if set.is_empty() {
                                self.prop_index.remove(&key);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Check uniqueness constraints for a node being created or updated.
    /// `exclude_node_id` is the node's own ID (to exclude self when checking
    /// updates); pass `None` for new node creation.
    pub(crate) fn check_uniqueness_constraints(
        &self,
        labels: &[String],
        properties: &IndexMap<String, Value>,
        exclude_node_id: Option<u64>,
    ) -> Result<(), StorageError> {
        for constraint in &self.constraints {
            if constraint.constraint_type != ConstraintType::Unique {
                continue;
            }
            if !labels.contains(&constraint.label) {
                continue;
            }
            let prop_val = match properties.get(&constraint.property) {
                Some(v) => v,
                None => continue,
            };
            // Check all existing nodes with the same label for a duplicate value.
            if let Some(label_nodes) = self.label_index.get(&constraint.label) {
                for &raw_id in label_nodes {
                    if exclude_node_id == Some(raw_id) {
                        continue;
                    }
                    if let Some(node) = self.nodes.get(&raw_id) {
                        if let Some(existing_val) = node.properties.get(&constraint.property) {
                            if existing_val == prop_val {
                                return Err(StorageError::ConstraintViolation {
                                    msg: format!(
                                        "Node(:{}) already exists with property `{}` = {:?}",
                                        constraint.label, constraint.property, prop_val
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn next_edge_id(&mut self, graph_name: &str) -> Result<u64, StorageError> {
        self.edge_seq = self
            .edge_seq
            .checked_add(1)
            .ok_or_else(|| StorageError::IdOverflow {
                graph: graph_name.to_string(),
            })?;
        Ok(self.edge_seq)
    }
}
