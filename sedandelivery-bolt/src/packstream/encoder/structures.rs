//! PackStream encoding for Structure values (tagged tuples).

use super::encode;
use crate::packstream::value::BoltValue;

pub(super) fn encode_structure(tag: u8, fields: &[BoltValue], buf: &mut Vec<u8>) {
    let field_count = if fields.len() > 15 {
        tracing::error!(
            tag,
            actual = fields.len(),
            "PackStream TinyStruct supports at most 15 fields; \
             truncating — this indicates a bug in the caller"
        );
        15
    } else {
        fields.len()
    };
    buf.push(0xB0 | field_count as u8);
    buf.push(tag);
    for field in &fields[..field_count] {
        encode(field, buf);
    }
}
