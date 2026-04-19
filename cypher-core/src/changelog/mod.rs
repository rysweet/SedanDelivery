//! Change log types for cross-backend synchronisation.
//!
//! # How it works
//!
//! [`ChangeLogStore<S>`] wraps any [`GraphStore`](crate::storage::GraphStore)
//! (typically `InMemoryGraphStore`) and intercepts every write operation.
//! After a write succeeds on the inner store the operation is recorded as a
//! [`ChangeEvent`] in an in-process pending queue.
//!
//! The Redis module command handler drains the queue after each successful
//! `GQL.MUTATE` / `GQL.CREATE-GRAPH` / `GQL.DELETE-GRAPH` command and appends
//! one Valkey stream entry per event via `ctx.call("XADD", ...)`.  This is a
//! module-internal call — no TCP socket, no deadlock.
//!
//! `sedandelivery-fabric-sync` (a separate process, optional) consumes the stream via
//! `XREADGROUP` and replays each event against `FabricGraphStore`.
//!
//! # Property encoding
//!
//! [`ChangeEvent`] stores properties as `serde_json::Map<String, serde_json::Value>`
//! using the canonical tagged-string encoding defined in [`crate::serde_value`].
//! This keeps [`ChangeEvent`] trivially `Serialize + Deserialize` via serde,
//! while preserving full round-trip fidelity for temporal, spatial, and list
//! values.
//!
//! # Enabling the changelog
//!
//! Set the `GQL_FABRIC_CHANGELOG=1` environment variable (or set any
//! `GQL_FABRIC_ENDPOINT` / `GQL_FABRIC_*` variables) before loading the module.
//! When disabled, [`ChangeLogStore::drain_events`] returns an empty `Vec` and
//! the module command handlers skip the `XADD` calls entirely — zero overhead.
//!
//! # Stream key
//!
//! All events are written to the Valkey stream `__sd2:changelog`.
//!
//! # Module layout
//!
//! - [`stream`] — stream key constants and per-graph stream key helper
//! - [`event`] — the [`ChangeEvent`] enum and its accessors
//! - [`props`] — JSON property map encoding helpers
//! - [`store`] — [`ChangeLogStore`] decorator and its `GraphStore` impl

mod event;
mod props;
mod store;
mod stream;

#[cfg(test)]
mod tests;

pub use event::ChangeEvent;
pub use props::json_map_to_props;
pub use store::ChangeLogStore;
pub use stream::{changelog_stream_for_graph, CHANGELOG_STREAM};
