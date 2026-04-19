//! `MERGE (a)-[r:R]->(b)` relationship match-or-create implementation.
//!
//! Split into focused submodules:
//! - [`batch`] batched fast-path used when no `ON CREATE`/`ON MATCH`
//!   clauses are present; deduplicates identical pending creates within
//!   a single MERGE pipeline.
//! - [`dispatch`] per-row path that handles `ON CREATE`/`ON MATCH`
//!   side-effects in addition to dispatching to the batched fast-path
//!   when applicable.

mod batch;
mod dispatch;
