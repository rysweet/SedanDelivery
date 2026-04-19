//! Tests for depth limits and pre-allocation safety caps.

use super::{decode, BoltValue, DecodeError};

// ── Depth limit tests ──────────────────────────────────────────────

/// Build `depth` levels of nested TinyList(1 element), with a Null at the
/// innermost level: 0x91 0x91 ... 0x91 0xC0
#[allow(clippy::same_item_push)]
fn nested_list_bytes(depth: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(depth + 1);
    for _ in 0..depth {
        buf.push(0x91); // TinyList with 1 element
    }
    buf.push(0xC0); // Null at the innermost level
    buf
}

/// Build `depth` levels of nested TinyMap(1 entry), with a Null value at
/// the innermost level: 0xA1 0x80 0xA1 0x80 ... 0xA1 0x80 0xC0
/// Each map has key="" (0x80 = TinyString of length 0) and the value is
/// the next nested map (or Null at the bottom).
fn nested_map_bytes(depth: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(depth * 2 + 1);
    for _ in 0..depth {
        buf.push(0xA1); // TinyMap with 1 entry
        buf.push(0x80); // key: empty string
    }
    buf.push(0xC0); // Null value at the innermost level
    buf
}

#[test]
fn test_max_depth_exceeded() {
    // 129 levels of nesting exceeds MAX_DECODE_DEPTH (128).
    // The top-level decode starts at depth 0, each TinyList adds +1.
    // After 128 TinyLists the depth is 128, and the next decode_inner
    // call at depth 129 triggers the > MAX_DECODE_DEPTH check.
    let buf = nested_list_bytes(129);
    let err = decode(&buf).unwrap_err();
    assert!(
        matches!(err, DecodeError::MaxDepthExceeded(128)),
        "expected MaxDepthExceeded(128), got: {:?}",
        err
    );
}

#[test]
fn test_depth_128_succeeds() {
    // Exactly 128 levels of nesting should succeed. The outermost
    // decode starts at depth 0, and the innermost Null is decoded
    // at depth 128 which is == MAX_DECODE_DEPTH (the check is >).
    let buf = nested_list_bytes(128);
    let result = decode(&buf);
    assert!(result.is_ok(), "128-deep nesting should decode OK");
}

#[test]
fn test_nested_map_depth_exceeded() {
    // Maps also increment depth. 129 nested maps should trigger
    // the depth limit just like lists do.
    let buf = nested_map_bytes(129);
    let err = decode(&buf).unwrap_err();
    assert!(
        matches!(err, DecodeError::MaxDepthExceeded(128)),
        "expected MaxDepthExceeded(128) for nested maps, got: {:?}",
        err
    );
}

// ── MAX_PREALLOC tests ────────────────────────────────────────────

/// Verify that `MAX_PREALLOC` caps the initial allocation for lists.
/// A List32 header claiming 1 million elements must not allocate
/// 1M `BoltValue` slots up-front; the `with_capacity` call should
/// be capped at `MAX_PREALLOC` (4096). The decode will fail with
/// UnexpectedEof because the buffer only provides a few elements,
/// but the key assertion is that the decoder does not OOM.
#[test]
#[allow(clippy::same_item_push)]
fn test_max_prealloc_caps_list_capacity() {
    let claimed_count: u32 = 1_000_000;
    let actual_elements: usize = 10;

    let mut buf = Vec::new();
    buf.push(0xD6); // List32 marker
    buf.extend_from_slice(&claimed_count.to_be_bytes());
    for _ in 0..actual_elements {
        buf.push(0xC0); // Null
    }

    let err = decode(&buf).unwrap_err();
    assert!(
        matches!(err, DecodeError::UnexpectedEof { .. }),
        "expected UnexpectedEof (not OOM) for over-claimed List32, got: {:?}",
        err
    );
}

/// Same test for Map32 -- claimed count is huge, but MAX_PREALLOC
/// prevents excessive pre-allocation.
#[test]
fn test_max_prealloc_caps_map_capacity() {
    let claimed_count: u32 = 1_000_000;
    let actual_entries: usize = 5;

    let mut buf = Vec::new();
    buf.push(0xDA); // Map32 marker
    buf.extend_from_slice(&claimed_count.to_be_bytes());
    for _ in 0..actual_entries {
        buf.push(0x80); // TinyString("")
        buf.push(0xC0); // Null
    }

    let err = decode(&buf).unwrap_err();
    assert!(
        matches!(err, DecodeError::UnexpectedEof { .. }),
        "expected UnexpectedEof (not OOM) for over-claimed Map32, got: {:?}",
        err
    );
}

/// Verify that a list with exactly MAX_PREALLOC elements decodes
/// correctly -- the capacity cap must not truncate the result.
#[test]
#[allow(clippy::same_item_push)]
fn test_list_at_max_prealloc_boundary_succeeds() {
    let count = super::super::config::MAX_PREALLOC; // 4096

    let mut buf = Vec::new();
    buf.push(0xD5); // List16 marker
    buf.extend_from_slice(&(count as u16).to_be_bytes());
    for _ in 0..count {
        buf.push(0xC0); // Null
    }

    let (val, consumed) = decode(&buf).expect("decode should succeed for MAX_PREALLOC elements");
    assert_eq!(consumed, buf.len());
    match val {
        BoltValue::List(items) => assert_eq!(items.len(), count),
        _ => panic!("expected List"),
    }
}
