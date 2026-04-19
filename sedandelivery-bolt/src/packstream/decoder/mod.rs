//! PackStream decoder.
//!
//! Decodes a single `BoltValue` from a byte buffer following the PackStream
//! v1/v2 wire format used by the Neo4j Bolt protocol.
//!
//! The module is split into:
//! - [`error`]: [`DecodeError`] enum and the `need()` length-check helper.
//! - [`config`]: depth-limit and pre-allocation safety knobs.
//! - [`containers`]: list and map decoders (which recurse via `decode_inner`).
//!
//! The top-level marker dispatch lives in this file so the marker `match` and
//! its required helpers stay co-located.

mod config;
mod containers;
mod error;

#[cfg(test)]
mod tests;

pub use error::DecodeError;

use super::value::BoltValue;
use config::max_decode_depth;
use containers::{decode_list, decode_map};
use error::need;

/// Decode one `BoltValue` from the front of `buf`.
///
/// Returns `(value, bytes_consumed)` on success.
pub fn decode(buf: &[u8]) -> Result<(BoltValue, usize), DecodeError> {
    decode_inner(buf, 0)
}

pub(super) fn decode_inner(buf: &[u8], depth: usize) -> Result<(BoltValue, usize), DecodeError> {
    let max_depth = max_decode_depth();
    if depth > max_depth {
        return Err(DecodeError::MaxDepthExceeded(max_depth));
    }
    if buf.is_empty() {
        return Err(DecodeError::UnexpectedEof {
            offset: 0,
            needed: 1,
            available: 0,
        });
    }

    let marker = buf[0];
    match marker {
        // ── Null ────────────────────────────────────────────────────
        0xC0 => Ok((BoltValue::Null, 1)),

        // ── Bool ────────────────────────────────────────────────────
        0xC2 => Ok((BoltValue::Bool(false), 1)),
        0xC3 => Ok((BoltValue::Bool(true), 1)),

        // ── Float64 ─────────────────────────────────────────────────
        0xC1 => {
            need(buf, 1, 8)?;
            let f = f64::from_be_bytes(buf[1..9].try_into().unwrap()); // safe: need() verified 8 bytes
            Ok((BoltValue::Float(f), 9))
        }

        // ── Int8 ────────────────────────────────────────────────────
        0xC8 => {
            need(buf, 1, 1)?;
            let v = buf[1] as i8 as i64;
            Ok((BoltValue::Int(v), 2))
        }

        // ── Int16 ───────────────────────────────────────────────────
        0xC9 => {
            need(buf, 1, 2)?;
            let v = i16::from_be_bytes([buf[1], buf[2]]) as i64;
            Ok((BoltValue::Int(v), 3))
        }

        // ── Int32 ───────────────────────────────────────────────────
        0xCA => {
            need(buf, 1, 4)?;
            let v = i32::from_be_bytes(buf[1..5].try_into().unwrap()) as i64; // safe: need() verified 4 bytes
            Ok((BoltValue::Int(v), 5))
        }

        // ── Int64 ───────────────────────────────────────────────────
        0xCB => {
            need(buf, 1, 8)?;
            let v = i64::from_be_bytes(buf[1..9].try_into().unwrap()); // safe: need() verified 8 bytes
            Ok((BoltValue::Int(v), 9))
        }

        // ── Bytes8 ──────────────────────────────────────────────────
        0xCC => {
            need(buf, 1, 1)?;
            let len = buf[1] as usize;
            need(buf, 2, len)?;
            let data = buf[2..2 + len].to_vec();
            Ok((BoltValue::Bytes(data), 2 + len))
        }

        // ── Bytes16 ─────────────────────────────────────────────────
        0xCD => {
            need(buf, 1, 2)?;
            let len = u16::from_be_bytes([buf[1], buf[2]]) as usize;
            need(buf, 3, len)?;
            let data = buf[3..3 + len].to_vec();
            Ok((BoltValue::Bytes(data), 3 + len))
        }

        // ── Bytes32 ─────────────────────────────────────────────────
        0xCE => {
            need(buf, 1, 4)?;
            let len = u32::from_be_bytes(buf[1..5].try_into().unwrap()) as usize; // safe: need() verified 4 bytes
            need(buf, 5, len)?;
            let data = buf[5..5 + len].to_vec();
            Ok((BoltValue::Bytes(data), 5 + len))
        }

        // ── String8 ─────────────────────────────────────────────────
        0xD0 => {
            need(buf, 1, 1)?;
            let len = buf[1] as usize;
            need(buf, 2, len)?;
            let s = String::from_utf8(buf[2..2 + len].to_vec())?;
            Ok((BoltValue::String(s), 2 + len))
        }

        // ── String16 ────────────────────────────────────────────────
        0xD1 => {
            need(buf, 1, 2)?;
            let len = u16::from_be_bytes([buf[1], buf[2]]) as usize;
            need(buf, 3, len)?;
            let s = String::from_utf8(buf[3..3 + len].to_vec())?;
            Ok((BoltValue::String(s), 3 + len))
        }

        // ── String32 ────────────────────────────────────────────────
        0xD2 => {
            need(buf, 1, 4)?;
            let len = u32::from_be_bytes(buf[1..5].try_into().unwrap()) as usize; // safe: need() verified 4 bytes
            need(buf, 5, len)?;
            let s = String::from_utf8(buf[5..5 + len].to_vec())?;
            Ok((BoltValue::String(s), 5 + len))
        }

        // ── List8 ───────────────────────────────────────────────────
        0xD4 => {
            need(buf, 1, 1)?;
            let count = buf[1] as usize;
            decode_list(buf, 2, count, depth)
        }

        // ── List16 ──────────────────────────────────────────────────
        0xD5 => {
            need(buf, 1, 2)?;
            let count = u16::from_be_bytes([buf[1], buf[2]]) as usize;
            decode_list(buf, 3, count, depth)
        }

        // ── List32 ──────────────────────────────────────────────────
        0xD6 => {
            need(buf, 1, 4)?;
            let count = u32::from_be_bytes(buf[1..5].try_into().unwrap()) as usize; // safe: need() verified 4 bytes
            decode_list(buf, 5, count, depth)
        }

        // ── Map8 ────────────────────────────────────────────────────
        0xD8 => {
            need(buf, 1, 1)?;
            let count = buf[1] as usize;
            decode_map(buf, 2, count, depth)
        }

        // ── Map16 ───────────────────────────────────────────────────
        0xD9 => {
            need(buf, 1, 2)?;
            let count = u16::from_be_bytes([buf[1], buf[2]]) as usize;
            decode_map(buf, 3, count, depth)
        }

        // ── Map32 ───────────────────────────────────────────────────
        0xDA => {
            need(buf, 1, 4)?;
            let count = u32::from_be_bytes(buf[1..5].try_into().unwrap()) as usize; // safe: need() verified 4 bytes
            decode_map(buf, 5, count, depth)
        }

        // ── TinyString (0x80..=0x8F) ────────────────────────────────
        m if (0x80..=0x8F).contains(&m) => {
            let len = (m & 0x0F) as usize;
            need(buf, 1, len)?;
            let s = String::from_utf8(buf[1..1 + len].to_vec())?;
            Ok((BoltValue::String(s), 1 + len))
        }

        // ── TinyList (0x90..=0x9F) ──────────────────────────────────
        m if (0x90..=0x9F).contains(&m) => {
            let count = (m & 0x0F) as usize;
            decode_list(buf, 1, count, depth)
        }

        // ── TinyMap (0xA0..=0xAF) ───────────────────────────────────
        m if (0xA0..=0xAF).contains(&m) => {
            let count = (m & 0x0F) as usize;
            decode_map(buf, 1, count, depth)
        }

        // ── Structure (0xB0..=0xBF) ─────────────────────────────────
        m if (0xB0..=0xBF).contains(&m) => {
            let nfields = (m & 0x0F) as usize;
            need(buf, 1, 1)?; // need at least the tag byte
            let tag = buf[1];
            let mut offset = 2;
            let mut fields = Vec::with_capacity(nfields);
            for _ in 0..nfields {
                let (val, consumed) = decode_inner(&buf[offset..], depth + 1)?;
                fields.push(val);
                offset += consumed;
            }
            Ok((BoltValue::Structure { tag, fields }, offset))
        }

        // ── TinyInt positive (0x00..=0x7F) ──────────────────────────
        m if m <= 0x7F => Ok((BoltValue::Int(m as i64), 1)),

        // ── TinyInt negative (0xF0..=0xFF) → -16..=-1 ──────────────
        m if m >= 0xF0 => Ok((BoltValue::Int(m as i8 as i64), 1)),

        // ── Reserved / unknown ──────────────────────────────────────
        _ => Err(DecodeError::InvalidMarker(marker)),
    }
}
