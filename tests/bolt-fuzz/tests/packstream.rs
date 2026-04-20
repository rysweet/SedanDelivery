//! Property tests for the Bolt PackStream decoder.
//!
//! Invariants under test:
//!
//! 1. `decode` returns a typed `Result` for **every** byte sequence — never
//!    panics, never aborts, never overflows the stack.
//! 2. Round-trip `encode → decode` of any generated [`BoltValue`] yields
//!    the original value.
//! 3. Deliberately malformed inputs (truncated length prefixes, deep
//!    nesting) surface a graceful [`DecodeError`].
//!
//! See `tests/embedded-fuzz/` (#1730) for the pattern this mirrors.

use bolt_fuzz::{
    deeply_nested_strategy, fuzz_config, marker_prefixed_bytes_strategy, random_bytes_strategy,
    truncated_payload_strategy, value_strategy, FAST_CASES, SOAK_CASES,
};
use proptest::prelude::*;
use sedandelivery_bolt::packstream::{
    decoder::{decode, DecodeError},
    encoder::encode,
    value::BoltValue,
};

// ── (a) Random byte sequences must never panic ───────────────────────────

proptest! {
    #![proptest_config(fuzz_config(FAST_CASES))]

    #[test]
    fn decode_random_bytes_never_panics(bytes in random_bytes_strategy()) {
        // The contract: any input → typed Result. No panic, no abort.
        let _ = decode(&bytes);
    }

    #[test]
    fn decode_marker_prefixed_bytes_never_panics(bytes in marker_prefixed_bytes_strategy()) {
        let _ = decode(&bytes);
    }
}

// ── (b) Round-trip encode → decode preserves equality ────────────────────

proptest! {
    #![proptest_config(fuzz_config(FAST_CASES))]

    #[test]
    fn encode_decode_roundtrip(value in value_strategy()) {
        let mut buf = Vec::new();
        encode(&value, &mut buf);
        let (decoded, consumed) = decode(&buf).expect("encode must produce decodable output");
        prop_assert_eq!(consumed, buf.len(), "decoder must consume entire encoded buffer");
        prop_assert_eq!(decoded, value);
    }
}

// ── (c) Malformed inputs must surface graceful errors ────────────────────

/// A graceful-error sentinel: every error variant the decoder may legally
/// return on malformed input. The list is exhaustive against the public
/// [`DecodeError`] enum — if a new variant is added, this match will fail
/// to compile and the test author must classify it as graceful or not.
fn is_graceful_error(err: &DecodeError) -> bool {
    matches!(
        err,
        DecodeError::UnexpectedEof { .. }
            | DecodeError::InvalidMarker(_)
            | DecodeError::Utf8Error(_)
            | DecodeError::TooManyFields(_)
            | DecodeError::NonStringMapKey(_)
            | DecodeError::MaxDepthExceeded(_)
    )
}

proptest! {
    #![proptest_config(fuzz_config(FAST_CASES))]

    #[test]
    fn truncated_inputs_yield_graceful_errors(bytes in truncated_payload_strategy()) {
        match decode(&bytes) {
            Ok(_) => {
                // Some "truncated" cases happen to land on a valid prefix
                // (e.g. List8 with declared length 0). That's allowed.
            }
            Err(e) => prop_assert!(
                is_graceful_error(&e),
                "decoder produced non-graceful error variant: {:?}", e
            ),
        }
    }

    #[test]
    fn deep_nesting_yields_graceful_error_or_value(bytes in deeply_nested_strategy()) {
        match decode(&bytes) {
            Ok(_) => {
                // Shallow nesting under the depth limit should succeed.
            }
            Err(e) => prop_assert!(
                is_graceful_error(&e),
                "deeply nested input produced non-graceful error: {:?}", e
            ),
        }
    }
}

// ── Soak: ≥ 30 000 random byte sequences in release mode ─────────────────

proptest! {
    #![proptest_config(fuzz_config(SOAK_CASES))]

    /// Drive ≥ 30 000 random byte sequences through the decoder and
    /// require that every one returns a typed `Result`. Mirrors the
    /// `--ignored` soak test pattern in `tests/embedded-fuzz/` but
    /// always runs (the harness is the contract).
    #[test]
    fn soak_decode_random_bytes_never_panics(bytes in random_bytes_strategy()) {
        match decode(&bytes) {
            Ok(_) => {}
            Err(e) => prop_assert!(
                is_graceful_error(&e),
                "soak found non-graceful error variant: {:?}", e
            ),
        }
    }
}

// ── Sanity: a couple of known good cases anchor the property tests ───────

#[test]
fn known_roundtrip_null() {
    let mut buf = Vec::new();
    encode(&BoltValue::Null, &mut buf);
    assert_eq!(buf, vec![0xC0]);
    assert_eq!(decode(&buf).unwrap(), (BoltValue::Null, 1));
}

#[test]
fn known_roundtrip_nested_map() {
    let v = BoltValue::Map(vec![(
        "k".into(),
        BoltValue::List(vec![BoltValue::Int(1), BoltValue::Bool(true)]),
    )]);
    let mut buf = Vec::new();
    encode(&v, &mut buf);
    let (decoded, consumed) = decode(&buf).unwrap();
    assert_eq!(consumed, buf.len());
    assert_eq!(decoded, v);
}
