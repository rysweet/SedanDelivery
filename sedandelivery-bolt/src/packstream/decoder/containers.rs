use super::config::MAX_PREALLOC;
use super::decode_inner;
use super::error::DecodeError;
use crate::packstream::value::BoltValue;

/// Decode `count` list items starting at `start` in `buf`.
pub(super) fn decode_list(
    buf: &[u8],
    start: usize,
    count: usize,
    depth: usize,
) -> Result<(BoltValue, usize), DecodeError> {
    let mut offset = start;
    let mut items = Vec::with_capacity(count.min(MAX_PREALLOC));
    for _ in 0..count {
        let (val, consumed) = decode_inner(&buf[offset..], depth + 1)?;
        items.push(val);
        offset += consumed;
    }
    Ok((BoltValue::List(items), offset))
}

/// Decode `count` map entries starting at `start` in `buf`.
pub(super) fn decode_map(
    buf: &[u8],
    start: usize,
    count: usize,
    depth: usize,
) -> Result<(BoltValue, usize), DecodeError> {
    let mut offset = start;
    let mut entries = Vec::with_capacity(count.min(MAX_PREALLOC));
    for _ in 0..count {
        let (key_val, key_consumed) = decode_inner(&buf[offset..], depth + 1)?;
        offset += key_consumed;
        let key = match key_val {
            BoltValue::String(s) => s,
            _ => return Err(DecodeError::NonStringMapKey(offset - key_consumed)),
        };
        let (val, val_consumed) = decode_inner(&buf[offset..], depth + 1)?;
        offset += val_consumed;
        entries.push((key, val));
    }
    Ok((BoltValue::Map(entries), offset))
}
