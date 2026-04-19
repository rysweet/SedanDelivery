//! Lib-side coverage smoke tests.
//!
//! Re-includes the comprehensive integration test files as lib tests so that
//! their code paths count toward `cargo llvm-cov -p cypher-core --lib`.
//! Pure additive: no production code is modified, no integration tests are
//! duplicated semantically — they are literally `#[path]`-included verbatim
//! after rebinding `cypher_core` to the current crate.
//!
//! These tests still also run as integration tests via the files under
//! `cypher-core/tests/`. Running them twice is acceptable; both targets are
//! fast enough.

#![cfg(test)]

#[path = "../tests/coverage_builtins.rs"]
mod coverage_builtins;

#[path = "../tests/coverage_planner.rs"]
mod coverage_planner;

#[path = "../tests/coverage_cached_store.rs"]
mod coverage_cached_store;

#[path = "../tests/end_to_end.rs"]
mod end_to_end;

#[path = "../tests/cypher_language_completeness.rs"]
mod cypher_language_completeness;

#[path = "../tests/data_integrity.rs"]
mod data_integrity;

#[path = "../tests/adversarial.rs"]
mod adversarial;

#[path = "../tests/persistence.rs"]
mod persistence;

#[path = "../tests/exists_interactive.rs"]
mod exists_interactive;

#[path = "../tests/issue_990.rs"]
mod issue_990;

#[path = "../tests/issue_1011.rs"]
mod issue_1011;

#[path = "../tests/stress.rs"]
mod stress;

#[allow(dead_code)]
#[path = "../tests/ldbc/mod.rs"]
mod ldbc;
