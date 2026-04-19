//! Gremlin executor: `projection` step implementations.
//!
//! Split into focused submodules per Gremlin operator:
//! - [`sample`]: `GremlinSample` — weighted reservoir sampling.
//! - [`map_slice`]: `GremlinMapSlice` — slice Map/List values.
//! - [`index`]: `GremlinIndex` — pair elements with positional indices.
//! - [`project`]: `GremlinProject` — evaluate sub-traversals per key.
//! - [`barrier`]: `GremlinBarrier` — bulk synchronization with sack merge.

mod barrier;
mod index;
mod map_slice;
mod project;
mod sample;
