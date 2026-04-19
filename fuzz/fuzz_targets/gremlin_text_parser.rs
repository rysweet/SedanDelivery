#![no_main]
//! Fuzz target for the Gremlin text parser
//! (`gremlin_core::text_parser::parse_traversal`).
//!
//! Contract: for any UTF-8 input, the parser must return
//! `Result<_, TextParseError>` rather than panicking. Parameter resolution
//! is stubbed out to `None` — fuzzing focuses on the parser itself, not
//! parameter substitution behaviour.

use libfuzzer_sys::fuzz_target;
use std::panic::{catch_unwind, AssertUnwindSafe};

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let _ = gremlin_core::text_parser::parse_traversal(s, &|_name| None);
    }));
    if outcome.is_err() {
        panic!("gremlin text parser panicked on input ({} bytes)", s.len());
    }
});
