//! Test suite for the PackStream decoder, split by topic.

use super::{decode, DecodeError};
use crate::packstream::encoder;
use crate::packstream::value::BoltValue;

mod containers;
mod errors;
mod limits;
mod primitives;
mod strings_bytes;
mod structures;

/// Helper: encode then decode, assert roundtrip equality and all bytes consumed.
pub(super) fn roundtrip(val: &BoltValue) {
    let mut buf = Vec::new();
    encoder::encode(val, &mut buf);
    let (decoded, consumed) = decode(&buf).expect("decode failed");
    assert_eq!(consumed, buf.len(), "not all bytes consumed");
    assert_eq!(&decoded, val, "roundtrip mismatch");
}
