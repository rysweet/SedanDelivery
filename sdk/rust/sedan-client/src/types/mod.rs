//! Tagged value decoder and graph-specific types.
//!
//! This module is split by concept:
//! - [`value`]: core value enum (`SedanValue`) plus `Point` and `SedanPath`.
//! - [`params`]: parameter substitution into Cypher query strings.
//! - [`decode`]: tagged-string decoding from the SedanDelivery wire format.
//!
//! All public items are re-exported here so the original
//! `sedan_client::types::*` API is preserved.

mod decode;
mod params;
mod value;

pub use decode::decode_value;
pub use params::substitute_params;
pub use value::{Point, SedanPath, SedanValue};
