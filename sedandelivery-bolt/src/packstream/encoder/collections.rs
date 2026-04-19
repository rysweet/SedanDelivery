//! PackStream encoding for List and Map collection values.

use super::bytes_str::encode_string;
use super::encode;
use crate::packstream::value::BoltValue;

pub(super) fn encode_list(items: &[BoltValue], buf: &mut Vec<u8>) {
    let len = items.len();
    if len <= 15 {
        buf.push(0x90 | len as u8);
    } else if len <= 255 {
        buf.push(0xD4);
        buf.push(len as u8);
    } else if len <= 65_535 {
        buf.push(0xD5);
        buf.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        buf.push(0xD6);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
    }
    for item in items {
        encode(item, buf);
    }
}

pub(super) fn encode_map(entries: &[(String, BoltValue)], buf: &mut Vec<u8>) {
    let len = entries.len();
    if len <= 15 {
        buf.push(0xA0 | len as u8);
    } else if len <= 255 {
        buf.push(0xD8);
        buf.push(len as u8);
    } else if len <= 65_535 {
        buf.push(0xD9);
        buf.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        buf.push(0xDA);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
    }
    for (key, val) in entries {
        encode_string(key, buf);
        encode(val, buf);
    }
}
