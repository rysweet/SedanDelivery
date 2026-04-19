#![no_main]
//! Fuzz target for the openCypher parser (`cypher_core::parser::parse_query`).
//!
//! The contract under test is: for any UTF-8 input, the parser must return a
//! `Result<_, ParseError>` rather than panicking, aborting, or looping
//! indefinitely. Non-UTF-8 input is silently dropped — the parser only
//! consumes `&str`.
//!
//! A panic surfaced here is a real bug: file an issue with the offending
//! input (libFuzzer will write a `crash-*` artifact next to the corpus).

use libfuzzer_sys::fuzz_target;
use std::panic::{catch_unwind, AssertUnwindSafe};

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let _ = cypher_core::parser::parse_query(s);
    }));
    if outcome.is_err() {
        // Re-raise so libFuzzer records the crash artifact.
        panic!("cypher parser panicked on input ({} bytes)", s.len());
    }
});
