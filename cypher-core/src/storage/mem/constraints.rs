//! Constraint management helpers for the in-memory backend.
//!
//! Inherent helpers invoked by the `GraphStore` impl in `mod.rs`. Keeping the
//! validation logic here keeps `mod.rs` focused on trait wiring.

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::StorageError;
use crate::storage::{ConstraintDescriptor, ConstraintType};

use super::store::InMemoryGraphStore;

pub(crate) fn list_constraints_impl(
    store: &InMemoryGraphStore,
    graph: &str,
) -> Result<Vec<ConstraintDescriptor>, StorageError> {
    let snap = store.read_snapshot()?;
    let g = InMemoryGraphStore::require_graph(&snap, graph)?;
    Ok(g.constraints.iter().cloned().collect())
}

pub(crate) fn create_constraint_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    label: &str,
    property: &str,
    constraint_type: ConstraintType,
) -> Result<(), StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;

    let descriptor = ConstraintDescriptor {
        label: label.to_string(),
        property: property.to_string(),
        constraint_type: constraint_type.clone(),
    };

    if g.constraints.contains(&descriptor) {
        return Err(StorageError::Backend {
            msg: format!(
                "Constraint already exists for :{}.{} ({})",
                label, property, constraint_type
            ),
        });
    }

    // Validate existing data does not violate the constraint.
    if constraint_type == ConstraintType::Unique {
        if let Some(label_nodes) = g.label_index.get(label) {
            // Collect (value_debug_string, raw_id) pairs for duplicate detection.
            // We use the Debug representation as a surrogate key since Value
            // does not implement Hash/Eq.
            let mut seen: HashMap<String, u64> = HashMap::new();
            for &raw_id in label_nodes {
                if let Some(node) = g.nodes.get(&raw_id) {
                    if let Some(val) = node.properties.get(property) {
                        let key = format!("{:?}", val);
                        if let Some(&first_id) = seen.get(&key) {
                            return Err(StorageError::ConstraintViolation {
                                msg: format!(
                                    "Cannot create UNIQUE constraint on :{}.{} — \
                                     nodes #{} and #{} both have value {:?}",
                                    label, property, first_id, raw_id, val
                                ),
                            });
                        }
                        seen.insert(key, raw_id);
                    }
                }
            }
        }
    }

    g.constraints.insert(descriptor);
    Ok(())
}

pub(crate) fn drop_constraint_impl(
    store: &InMemoryGraphStore,
    graph: &str,
    label: &str,
    property: &str,
) -> Result<(), StorageError> {
    let mut guard = store.write()?;
    let map = Arc::make_mut(&mut guard);
    let g = InMemoryGraphStore::require_graph_mut(map, graph)?;

    // Remove any constraint matching label+property (regardless of type).
    let before = g.constraints.len();
    g.constraints
        .retain(|c| !(c.label == label && c.property == property));
    if g.constraints.len() == before {
        return Err(StorageError::Backend {
            msg: format!("No constraint found for :{}.{}", label, property),
        });
    }
    Ok(())
}
