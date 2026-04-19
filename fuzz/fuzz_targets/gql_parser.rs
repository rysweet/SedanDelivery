#![no_main]
//! Fuzz target for the GQL (ISO/IEC 39075) parser
//! (`gql_engine::parser::parse_gql`).
//!
//! Contract: for any UTF-8 input, the parser must return
//! `Result<_, GqlParseError>` rather than panicking.

use libfuzzer_sys::fuzz_target;
use std::panic::{catch_unwind, AssertUnwindSafe};

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let _ = gql_engine::parser::parse_gql(s);
    }));
    if outcome.is_err() {
        panic!("gql parser panicked on input ({} bytes)", s.len());
    }
});
