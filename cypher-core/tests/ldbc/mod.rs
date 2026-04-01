//! LDBC Social Network Benchmark test suite.
//!
//! Phase 1 of the comprehensive end-to-end validation plan (issue #189).
//!
//! Uses a synthetic LDBC-schema dataset (see `loader`) to validate that
//! the GQL engine correctly handles the full spectrum of real-world Cypher
//! patterns without requiring an external data download.
//!
//! Modules:
//!  - `loader`             — Synthetic dataset builder
//!  - `interactive_reads`  — IC1–IC14 complex read queries
//!  - `interactive_writes` — IU1–IU8 insert + ID1–ID8 delete operations

pub mod interactive_reads;
pub mod interactive_writes;
pub mod loader;
