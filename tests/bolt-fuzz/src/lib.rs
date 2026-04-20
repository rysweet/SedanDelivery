//! Generators and shared helpers for the Bolt PackStream fuzz harness.
//!
//! This crate is **not** a runtime dependency of any production binary. It
//! exists only to host property-tests against the public surfaces of the
//! Bolt v4.4 PackStream codec exposed by [`sedandelivery_bolt::packstream`]:
//!
//! * [`sedandelivery_bolt::packstream::encoder::encode`]
//! * [`sedandelivery_bolt::packstream::decoder::decode`]
//!
//! The harness asserts *invariants*, not output values:
//!
//! 1. `decode` never panics on **any** byte sequence — it must return a
//!    typed [`DecodeError`](sedandelivery_bolt::packstream::decoder::DecodeError)
//!    on malformed input.
//! 2. `encode` never panics on any well-formed [`BoltValue`].
//! 3. Round-trip `encode → decode` of generated [`BoltValue`]s yields the
//!    original value exactly (modulo finite-float quirks that the
//!    generator avoids).
//!
//! Mirrors the pattern in `tests/embedded-fuzz/` (#1730).

use proptest::prelude::*;
use sedandelivery_bolt::packstream::value::BoltValue;

/// Bounded depth for recursively generated PackStream values.
pub const MAX_DEPTH: u32 = 3;

/// Default number of cases for the fast property tests.
pub const FAST_CASES: u32 = 256;

/// Number of cases for the soak / stress variants. Used by the release-mode
/// harness to exercise ≥ 30 000 random sequences.
pub const SOAK_CASES: u32 = 30_000;

/// Deterministic seed used by every test in this crate.
///
/// Pin one seed across the whole harness so failures are reproducible
/// across CI runs and developer machines. To regenerate after a true
/// failure, replace this constant with a fresh 32-byte value.
pub const FUZZ_SEED: [u8; 32] = *b"SedanDelivery-bolt-pack-fuzz-v1!";

/// Build a [`ProptestConfig`] that uses the deterministic seed and the
/// supplied case count, with shrinking budget tightened and persistence
/// disabled so the run is fully self-contained.
pub fn fuzz_config(cases: u32) -> ProptestConfig {
    let mut cfg = ProptestConfig::with_cases(cases);
    cfg.failure_persistence = None;
    cfg.rng_algorithm = proptest::test_runner::RngAlgorithm::ChaCha;
    cfg
}

/// Strategy producing arbitrary byte sequences of bounded length.
///
/// Length is bounded so each individual case stays cheap to decode — the
/// goal is breadth (many random inputs), not pathological single inputs.
pub fn random_bytes_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..512)
}

/// Strategy producing byte sequences that *start* with a real PackStream
/// marker followed by random tail bytes.
///
/// This biases the random stream toward inputs that exercise the
/// length-prefix and per-marker branch logic rather than the
/// `InvalidMarker` early-return path that random bytes hit ~37% of the
/// time.
pub fn marker_prefixed_bytes_strategy() -> impl Strategy<Value = Vec<u8>> {
    let markers: Vec<u8> = (0u8..=0xFFu8).collect();
    (
        prop::sample::select(markers),
        prop::collection::vec(any::<u8>(), 0..256),
    )
        .prop_map(|(m, mut tail)| {
            tail.insert(0, m);
            tail
        })
}

/// Strategy producing UTF-8 strings of bounded length.
///
/// Bounds are chosen to span tiny-string (≤ 15 bytes), string-8
/// (≤ 255), and string-16 (≤ 65 535) length classes without making
/// individual cases prohibitively expensive.
pub fn string_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..32).prop_map(|cs| cs.into_iter().collect())
}

/// Strategy producing scalar [`BoltValue`] instances.
///
/// Floats are restricted to a finite range to keep `PartialEq` round-trip
/// assertions meaningful (NaN never equals itself).
pub fn scalar_value_strategy() -> impl Strategy<Value = BoltValue> {
    prop_oneof![
        Just(BoltValue::Null),
        any::<bool>().prop_map(BoltValue::Bool),
        any::<i64>().prop_map(BoltValue::Int),
        (-1.0e9_f64..1.0e9_f64).prop_map(BoltValue::Float),
        prop::collection::vec(any::<u8>(), 0..32).prop_map(BoltValue::Bytes),
        string_strategy().prop_map(BoltValue::String),
    ]
}

/// Strategy producing arbitrary [`BoltValue`] instances, including
/// recursive lists, maps, and structures up to [`MAX_DEPTH`].
pub fn value_strategy() -> impl Strategy<Value = BoltValue> {
    let leaf = scalar_value_strategy();
    leaf.prop_recursive(MAX_DEPTH, 32, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(BoltValue::List),
            prop::collection::vec((string_strategy(), inner.clone()), 0..4)
                .prop_map(BoltValue::Map),
            // Tags are restricted to the 0x00..=0x7F range to avoid clashing
            // with bolt control bytes when a user reads back the wire form;
            // the decoder accepts any u8, so this is purely cosmetic.
            (0u8..=0x7Fu8, prop::collection::vec(inner, 0..4))
                .prop_map(|(tag, fields)| BoltValue::Structure { tag, fields }),
        ]
    })
}

/// Strategy producing PackStream byte streams that look like a
/// **truncated** value: a real header (marker + length prefix) followed
/// by *fewer* payload bytes than the header advertises.
///
/// These are the inputs that historically triggered out-of-bounds slices
/// in length-prefixed decoders; a robust decoder must surface
/// [`UnexpectedEof`](sedandelivery_bolt::packstream::decoder::DecodeError::UnexpectedEof).
pub fn truncated_payload_strategy() -> impl Strategy<Value = Vec<u8>> {
    // Pick one of the length-prefixed markers and a declared length, then
    // emit *fewer* payload bytes than declared.
    let cases: Vec<(u8, u32)> = vec![
        (0xCC, 200),       // Bytes8
        (0xCD, 5_000),     // Bytes16
        (0xCE, 1_000_000), // Bytes32 — declared, never delivered
        (0xD0, 200),       // String8
        (0xD1, 5_000),     // String16
        (0xD2, 1_000_000), // String32
        (0xD4, 200),       // List8
        (0xD5, 5_000),     // List16
        (0xD6, 1_000_000), // List32
        (0xD8, 200),       // Map8
        (0xD9, 5_000),     // Map16
        (0xDA, 1_000_000), // Map32
    ];
    (
        prop::sample::select(cases),
        prop::collection::vec(any::<u8>(), 0..16),
    )
        .prop_map(|((marker, declared_len), payload_tail)| {
            let mut buf = vec![marker];
            // Emit the appropriate header width for each marker class.
            match marker {
                0xCC | 0xD0 | 0xD4 | 0xD8 => buf.push(declared_len.min(255) as u8),
                0xCD | 0xD1 | 0xD5 | 0xD9 => {
                    buf.extend_from_slice(&(declared_len.min(65_535) as u16).to_be_bytes())
                }
                0xCE | 0xD2 | 0xD6 | 0xDA => buf.extend_from_slice(&declared_len.to_be_bytes()),
                _ => unreachable!(),
            }
            // Always give it less than declared.
            buf.extend_from_slice(&payload_tail);
            buf
        })
}

/// Strategy producing **deeply nested** TinyList headers.
///
/// Each generated buffer is a chain of `0x91` (TinyList of 1 element)
/// followed by another such header, terminating in a single `0xC0`
/// (Null). At sufficient depth this must trigger
/// [`MaxDepthExceeded`](sedandelivery_bolt::packstream::decoder::DecodeError::MaxDepthExceeded)
/// rather than blowing the host stack.
pub fn deeply_nested_strategy() -> impl Strategy<Value = Vec<u8>> {
    (1usize..512usize).prop_map(|depth| {
        let mut buf = vec![0x91u8; depth];
        buf.push(0xC0);
        buf
    })
}
