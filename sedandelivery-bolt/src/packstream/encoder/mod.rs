//! PackStream encoder: serialises [`BoltValue`] into the binary wire format.
//!
//! The implementation is split into topic-focused submodules:
//! - [`scalars`]: integer and float encoding
//! - [`bytes_str`]: string and byte-slice encoding
//! - [`collections`]: list and map encoding
//! - [`structures`]: tagged structure encoding

mod bytes_str;
mod collections;
mod scalars;
mod structures;

use super::value::BoltValue;

use bytes_str::{encode_bytes, encode_string};
use collections::{encode_list, encode_map};
use scalars::{encode_float, encode_int};
use structures::encode_structure;

/// Encode a `BoltValue` into PackStream binary format, appending bytes to `buf`.
pub fn encode(val: &BoltValue, buf: &mut Vec<u8>) {
    match val {
        BoltValue::Null => buf.push(0xC0),

        BoltValue::Bool(false) => buf.push(0xC2),
        BoltValue::Bool(true) => buf.push(0xC3),

        BoltValue::Int(n) => encode_int(*n, buf),
        BoltValue::Float(f) => encode_float(*f, buf),
        BoltValue::Bytes(data) => encode_bytes(data, buf),
        BoltValue::String(s) => encode_string(s, buf),
        BoltValue::List(items) => encode_list(items, buf),
        BoltValue::Map(entries) => encode_map(entries, buf),
        BoltValue::Structure { tag, fields } => encode_structure(*tag, fields, buf),
    }
}

#[cfg(test)]
mod tests_aggregate;
#[cfg(test)]
mod tests_scalars;
