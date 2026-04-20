//! Node CRUD operations for [`InMemoryGraphStore`].
//!
//! Split into focused submodules:
//! - [`create`]: node creation
//! - [`properties`]: property set/replace/remove
//! - [`labels`]: label add/remove
//! - [`delete`]: node deletion (with optional detach)

mod create;
mod delete;
mod labels;
mod properties;

#[cfg(test)]
mod tests;

pub(crate) use create::{create_node_impl, create_node_with_id_impl};
pub(crate) use delete::delete_node_impl;
pub(crate) use labels::{add_node_labels_impl, remove_node_labels_impl};
pub(crate) use properties::{
    remove_node_property_impl, replace_node_properties_impl, set_node_properties_impl,
};
