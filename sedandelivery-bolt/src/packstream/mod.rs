//! PackStream binary serialization format.
//!
//! Implements the PackStream v1 codec used by the Bolt protocol to encode
//! values on the wire.  The format supports typed scalars (null, bool, int,
//! float, string, bytes), composites (list, map), and tagged structures.

pub mod decoder;
pub mod encoder;
pub mod value;
