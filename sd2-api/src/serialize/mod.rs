//! Serialisation helpers: convert cypher-core types to `serde_json::Value`.
//!
//! Submodules:
//! - [`value`] — scalar / property value serialisation (`value_to_json`, `props_to_json`)
//! - [`entity`] — node / edge wire representations (`node_to_json`, `edge_to_json`)
//! - [`row`] — query result rows with node/edge resolution (`row_to_json`)

mod entity;
mod row;
mod value;

pub use entity::{edge_to_json, node_to_json};
pub use row::row_to_json;
pub use value::{props_to_json, value_to_json};
