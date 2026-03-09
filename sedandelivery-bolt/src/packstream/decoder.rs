use super::value::BoltValue;

/// Errors that can occur while decoding PackStream data.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("unexpected end of input: needed {needed} bytes at offset {offset} but only {available} available")]
    UnexpectedEof {
        offset: usize,
        needed: usize,
        available: usize,
    },
    #[error("invalid marker byte 0x{0:02X}")]
    InvalidMarker(u8),
    #[error("UTF-8 error: {0}")]
    Utf8Error(#[from] std::string::FromUtf8Error),
    #[error("structure has too many fields: {0}")]
    TooManyFields(usize),
    #[error("map key is not a string at offset {0}")]
    NonStringMapKey(usize),
    #[error("nesting depth exceeds maximum ({0})")]
    MaxDepthExceeded(usize),
}

/// Maximum nesting depth for recursive structures (lists, maps, structures).
/// Prevents stack overflow from maliciously crafted deeply-nested messages.
/// Configurable via `BOLT_MAX_DECODE_DEPTH` env var (default 128).
fn max_decode_depth() -> usize {
    static DEPTH: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *DEPTH.get_or_init(|| {
        std::env::var("BOLT_MAX_DECODE_DEPTH")
            .ok()
            .and_then(|v| match v.parse::<usize>() {
                Ok(val) if val > 0 => Some(val),
                _ => {
                    tracing::warn!(
                        "invalid BOLT_MAX_DECODE_DEPTH value, using default 128"
                    );
                    None
                }
            })
            .unwrap_or(128)
    })
}

/// Decode one `BoltValue` from the front of `buf`.
///
/// Returns `(value, bytes_consumed)` on success.
pub fn decode(buf: &[u8]) -> Result<(BoltValue, usize), DecodeError> {
    decode_inner(buf, 0)
}

fn decode_inner(buf: &[u8], depth: usize) -> Result<(BoltValue, usize), DecodeError> {
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

/// Check that `buf[offset..]` has at least `needed` bytes.
fn need(buf: &[u8], offset: usize, needed: usize) -> Result<(), DecodeError> {
    let available = buf.len().saturating_sub(offset);
    if available < needed {
        Err(DecodeError::UnexpectedEof {
            offset,
            needed,
            available,
        })
    } else {
        Ok(())
    }
}

/// Maximum pre-allocation for list/map capacity from untrusted wire data,
/// measured in **elements** (not bytes).
///
/// The actual element count is still honoured; this only caps the initial
/// `Vec::with_capacity` call to prevent a single malicious List32/Map32 header
/// from triggering a multi-GB allocation (CVE-class OOM DoS).
///
/// 4096 elements is a reasonable upper bound for typical Bolt messages (result
/// sets, parameter maps) while being small enough that the worst-case
/// allocation (4096 * size_of::<BoltValue>()) stays well under 1 MiB.
const MAX_PREALLOC: usize = 4096;

/// Decode `count` list items starting at `offset` in `buf`.
fn decode_list(
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

/// Decode `count` map entries starting at `offset` in `buf`.
fn decode_map(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packstream::encoder;

    /// Helper: encode then decode, assert roundtrip equality and all bytes consumed.
    fn roundtrip(val: &BoltValue) {
        let mut buf = Vec::new();
        encoder::encode(val, &mut buf);
        let (decoded, consumed) = decode(&buf).expect("decode failed");
        assert_eq!(consumed, buf.len(), "not all bytes consumed");
        assert_eq!(&decoded, val, "roundtrip mismatch");
    }

    // ── Null ────────────────────────────────────────────────────────────

    #[test]
    fn decode_null() {
        let (val, n) = decode(&[0xC0]).unwrap();
        assert_eq!(val, BoltValue::Null);
        assert_eq!(n, 1);
    }

    #[test]
    fn roundtrip_null() {
        roundtrip(&BoltValue::Null);
    }

    // ── Bool ────────────────────────────────────────────────────────────

    #[test]
    fn decode_false() {
        let (val, n) = decode(&[0xC2]).unwrap();
        assert_eq!(val, BoltValue::Bool(false));
        assert_eq!(n, 1);
    }

    #[test]
    fn decode_true() {
        let (val, n) = decode(&[0xC3]).unwrap();
        assert_eq!(val, BoltValue::Bool(true));
        assert_eq!(n, 1);
    }

    #[test]
    fn roundtrip_bool() {
        roundtrip(&BoltValue::Bool(true));
        roundtrip(&BoltValue::Bool(false));
    }

    // ── TinyInt ─────────────────────────────────────────────────────────

    #[test]
    fn decode_tinyint_zero() {
        let (val, n) = decode(&[0x00]).unwrap();
        assert_eq!(val, BoltValue::Int(0));
        assert_eq!(n, 1);
    }

    #[test]
    fn decode_tinyint_1() {
        let (val, _) = decode(&[0x01]).unwrap();
        assert_eq!(val, BoltValue::Int(1));
    }

    #[test]
    fn decode_tinyint_127() {
        let (val, _) = decode(&[0x7F]).unwrap();
        assert_eq!(val, BoltValue::Int(127));
    }

    #[test]
    fn decode_tinyint_neg1() {
        let (val, _) = decode(&[0xFF]).unwrap();
        assert_eq!(val, BoltValue::Int(-1));
    }

    #[test]
    fn decode_tinyint_neg16() {
        let (val, _) = decode(&[0xF0]).unwrap();
        assert_eq!(val, BoltValue::Int(-16));
    }

    // ── Int8 ────────────────────────────────────────────────────────────

    #[test]
    fn decode_int8_neg17() {
        let (val, n) = decode(&[0xC8, 0xEF]).unwrap();
        assert_eq!(val, BoltValue::Int(-17));
        assert_eq!(n, 2);
    }

    #[test]
    fn decode_int8_neg128() {
        let (val, _) = decode(&[0xC8, 0x80]).unwrap();
        assert_eq!(val, BoltValue::Int(-128));
    }

    // ── Int16 ───────────────────────────────────────────────────────────

    #[test]
    fn decode_int16_128() {
        let (val, n) = decode(&[0xC9, 0x00, 0x80]).unwrap();
        assert_eq!(val, BoltValue::Int(128));
        assert_eq!(n, 3);
    }

    #[test]
    fn decode_int16_neg129() {
        let (val, _) = decode(&[0xC9, 0xFF, 0x7F]).unwrap();
        assert_eq!(val, BoltValue::Int(-129));
    }

    #[test]
    fn decode_int16_32767() {
        let (val, _) = decode(&[0xC9, 0x7F, 0xFF]).unwrap();
        assert_eq!(val, BoltValue::Int(32767));
    }

    #[test]
    fn decode_int16_neg32768() {
        let (val, _) = decode(&[0xC9, 0x80, 0x00]).unwrap();
        assert_eq!(val, BoltValue::Int(-32768));
    }

    // ── Int32 ───────────────────────────────────────────────────────────

    #[test]
    fn decode_int32_32768() {
        let (val, n) = decode(&[0xCA, 0x00, 0x00, 0x80, 0x00]).unwrap();
        assert_eq!(val, BoltValue::Int(32768));
        assert_eq!(n, 5);
    }

    #[test]
    fn decode_int32_i32_max() {
        let (val, _) = decode(&[0xCA, 0x7F, 0xFF, 0xFF, 0xFF]).unwrap();
        assert_eq!(val, BoltValue::Int(i32::MAX as i64));
    }

    #[test]
    fn decode_int32_i32_min() {
        let (val, _) = decode(&[0xCA, 0x80, 0x00, 0x00, 0x00]).unwrap();
        assert_eq!(val, BoltValue::Int(i32::MIN as i64));
    }

    // ── Int64 ───────────────────────────────────────────────────────────

    #[test]
    fn decode_int64_max() {
        let mut data = vec![0xCB];
        data.extend_from_slice(&i64::MAX.to_be_bytes());
        let (val, n) = decode(&data).unwrap();
        assert_eq!(val, BoltValue::Int(i64::MAX));
        assert_eq!(n, 9);
    }

    #[test]
    fn decode_int64_min() {
        let mut data = vec![0xCB];
        data.extend_from_slice(&i64::MIN.to_be_bytes());
        let (val, _) = decode(&data).unwrap();
        assert_eq!(val, BoltValue::Int(i64::MIN));
    }

    // ── Roundtrip all integer boundaries ────────────────────────────────

    #[test]
    fn roundtrip_int_boundaries() {
        let values: Vec<i64> = vec![
            0,
            1,
            -1,
            127,
            -16,
            -17,
            -128,
            128,
            -129,
            32767,
            -32768,
            32768,
            -32769,
            i32::MAX as i64,
            i32::MIN as i64,
            i32::MAX as i64 + 1,
            i32::MIN as i64 - 1,
            i64::MAX,
            i64::MIN,
        ];
        for v in values {
            roundtrip(&BoltValue::Int(v));
        }
    }

    // ── Float ───────────────────────────────────────────────────────────

    #[test]
    fn decode_float_1_1() {
        let data = [0xC1, 0x3F, 0xF1, 0x99, 0x99, 0x99, 0x99, 0x99, 0x9A];
        let (val, n) = decode(&data).unwrap();
        assert_eq!(n, 9);
        match val {
            BoltValue::Float(f) => assert!((f - 1.1).abs() < f64::EPSILON),
            _ => panic!("expected Float"),
        }
    }

    #[test]
    fn roundtrip_float() {
        roundtrip(&BoltValue::Float(0.0));
        roundtrip(&BoltValue::Float(1.1));
        roundtrip(&BoltValue::Float(-999.999));
        roundtrip(&BoltValue::Float(f64::MAX));
        roundtrip(&BoltValue::Float(f64::MIN));
    }

    // ── String ──────────────────────────────────────────────────────────

    #[test]
    fn decode_empty_string() {
        let (val, n) = decode(&[0x80]).unwrap();
        assert_eq!(val, BoltValue::String(String::new()));
        assert_eq!(n, 1);
    }

    #[test]
    fn decode_tiny_string_hello() {
        let data = [0x85, 0x68, 0x65, 0x6C, 0x6C, 0x6F];
        let (val, n) = decode(&data).unwrap();
        assert_eq!(val, BoltValue::String("hello".into()));
        assert_eq!(n, 6);
    }

    #[test]
    fn roundtrip_string_tiers() {
        roundtrip(&BoltValue::String(String::new()));
        roundtrip(&BoltValue::String("a".into()));
        roundtrip(&BoltValue::String("abcdefghijklmno".into())); // 15 bytes
        roundtrip(&BoltValue::String("a".repeat(16))); // String8
        roundtrip(&BoltValue::String("a".repeat(255))); // String8 max
        roundtrip(&BoltValue::String("a".repeat(256))); // String16
    }

    #[test]
    fn roundtrip_string32() {
        roundtrip(&BoltValue::String("z".repeat(65_536)));
    }

    #[test]
    fn decode_string_utf8() {
        let s = "cafe\u{0301}"; // "cafe" + combining accent
        roundtrip(&BoltValue::String(s.into()));
    }

    // ── Bytes ───────────────────────────────────────────────────────────

    #[test]
    fn decode_empty_bytes() {
        let (val, n) = decode(&[0xCC, 0x00]).unwrap();
        assert_eq!(val, BoltValue::Bytes(vec![]));
        assert_eq!(n, 2);
    }

    #[test]
    fn roundtrip_bytes_tiers() {
        roundtrip(&BoltValue::Bytes(vec![]));
        roundtrip(&BoltValue::Bytes(vec![0xAB; 10]));
        roundtrip(&BoltValue::Bytes(vec![0x01; 255]));
        roundtrip(&BoltValue::Bytes(vec![0x02; 256]));
    }

    // ── List ────────────────────────────────────────────────────────────

    #[test]
    fn decode_empty_list() {
        let (val, n) = decode(&[0x90]).unwrap();
        assert_eq!(val, BoltValue::List(vec![]));
        assert_eq!(n, 1);
    }

    #[test]
    fn decode_tiny_list() {
        let data = [0x93, 0x01, 0x02, 0x03];
        let (val, n) = decode(&data).unwrap();
        assert_eq!(
            val,
            BoltValue::List(vec![
                BoltValue::Int(1),
                BoltValue::Int(2),
                BoltValue::Int(3),
            ])
        );
        assert_eq!(n, 4);
    }

    #[test]
    fn roundtrip_list_empty() {
        roundtrip(&BoltValue::List(vec![]));
    }

    #[test]
    fn roundtrip_list_nested() {
        let v = BoltValue::List(vec![
            BoltValue::List(vec![BoltValue::Int(1), BoltValue::Int(2)]),
            BoltValue::Null,
            BoltValue::String("test".into()),
        ]);
        roundtrip(&v);
    }

    #[test]
    fn roundtrip_list8() {
        let items: Vec<BoltValue> = (0..16).map(BoltValue::Int).collect();
        roundtrip(&BoltValue::List(items));
    }

    #[test]
    fn roundtrip_list16() {
        let items: Vec<BoltValue> = (0..256).map(BoltValue::Int).collect();
        roundtrip(&BoltValue::List(items));
    }

    // ── Map ─────────────────────────────────────────────────────────────

    #[test]
    fn decode_empty_map() {
        let (val, n) = decode(&[0xA0]).unwrap();
        assert_eq!(val, BoltValue::Map(vec![]));
        assert_eq!(n, 1);
    }

    #[test]
    fn decode_tiny_map_one_entry() {
        // {"one": 1} → A1 83 6F 6E 65 01
        let data = [0xA1, 0x83, 0x6F, 0x6E, 0x65, 0x01];
        let (val, n) = decode(&data).unwrap();
        assert_eq!(val, BoltValue::Map(vec![("one".into(), BoltValue::Int(1))]));
        assert_eq!(n, 6);
    }

    #[test]
    fn roundtrip_map() {
        let map = BoltValue::Map(vec![
            ("name".into(), BoltValue::String("Alice".into())),
            ("age".into(), BoltValue::Int(30)),
            ("active".into(), BoltValue::Bool(true)),
        ]);
        roundtrip(&map);
    }

    #[test]
    fn roundtrip_map8() {
        let entries: Vec<(String, BoltValue)> = (0..16)
            .map(|i| (format!("key_{}", i), BoltValue::Int(i)))
            .collect();
        roundtrip(&BoltValue::Map(entries));
    }

    // ── Structure ───────────────────────────────────────────────────────

    #[test]
    fn decode_structure_no_fields() {
        let (val, n) = decode(&[0xB0, 0x01]).unwrap();
        assert_eq!(
            val,
            BoltValue::Structure {
                tag: 0x01,
                fields: vec![]
            }
        );
        assert_eq!(n, 2);
    }

    #[test]
    fn decode_structure_with_fields() {
        // tag 0x71, one field: Int(42)
        let data = [0xB1, 0x71, 0x2A];
        let (val, n) = decode(&data).unwrap();
        assert_eq!(
            val,
            BoltValue::Structure {
                tag: 0x71,
                fields: vec![BoltValue::Int(42)]
            }
        );
        assert_eq!(n, 3);
    }

    #[test]
    fn roundtrip_structure() {
        let s = BoltValue::Structure {
            tag: 0x4E, // Node
            fields: vec![
                BoltValue::Int(123),
                BoltValue::List(vec![BoltValue::String("Person".into())]),
                BoltValue::Map(vec![
                    ("name".into(), BoltValue::String("Alice".into())),
                    ("age".into(), BoltValue::Int(30)),
                ]),
            ],
        };
        roundtrip(&s);
    }

    // ── Nested ──────────────────────────────────────────────────────────

    #[test]
    fn roundtrip_deeply_nested() {
        let v = BoltValue::Map(vec![(
            "data".into(),
            BoltValue::List(vec![
                BoltValue::Map(vec![
                    ("id".into(), BoltValue::Int(1)),
                    (
                        "tags".into(),
                        BoltValue::List(vec![
                            BoltValue::String("a".into()),
                            BoltValue::String("b".into()),
                        ]),
                    ),
                ]),
                BoltValue::Null,
                BoltValue::Bool(false),
                BoltValue::Float(2.72),
                BoltValue::Bytes(vec![0xDE, 0xAD]),
            ]),
        )]);
        roundtrip(&v);
    }

    // ── Error conditions ────────────────────────────────────────────────

    #[test]
    fn decode_empty_input() {
        let err = decode(&[]).unwrap_err();
        assert!(matches!(err, DecodeError::UnexpectedEof { .. }));
    }

    #[test]
    fn decode_truncated_int16() {
        // Int16 marker but only 1 payload byte
        let err = decode(&[0xC9, 0x00]).unwrap_err();
        assert!(matches!(err, DecodeError::UnexpectedEof { .. }));
    }

    #[test]
    fn decode_truncated_float() {
        let err = decode(&[0xC1, 0x00, 0x00]).unwrap_err();
        assert!(matches!(err, DecodeError::UnexpectedEof { .. }));
    }

    #[test]
    fn decode_truncated_string() {
        // TinyString of length 5 but only 2 bytes of payload
        let err = decode(&[0x85, b'h', b'i']).unwrap_err();
        assert!(matches!(err, DecodeError::UnexpectedEof { .. }));
    }

    #[test]
    fn decode_invalid_utf8_string() {
        // TinyString of length 2 with invalid UTF-8
        let err = decode(&[0x82, 0xFF, 0xFE]).unwrap_err();
        assert!(matches!(err, DecodeError::Utf8Error(_)));
    }

    #[test]
    fn decode_non_string_map_key() {
        // Map with 1 entry, key is Int(1) instead of String
        let err = decode(&[0xA1, 0x01, 0x02]).unwrap_err();
        assert!(matches!(err, DecodeError::NonStringMapKey(_)));
    }

    #[test]
    fn decode_invalid_marker() {
        // 0xCF is reserved/unused
        let err = decode(&[0xCF]).unwrap_err();
        assert!(matches!(err, DecodeError::InvalidMarker(0xCF)));
    }

    // ── Spec hex examples ───────────────────────────────────────────────

    #[test]
    fn spec_hex_null() {
        let (val, _) = decode(&[0xC0]).unwrap();
        assert_eq!(val, BoltValue::Null);
    }

    #[test]
    fn spec_hex_false() {
        let (val, _) = decode(&[0xC2]).unwrap();
        assert_eq!(val, BoltValue::Bool(false));
    }

    #[test]
    fn spec_hex_true() {
        let (val, _) = decode(&[0xC3]).unwrap();
        assert_eq!(val, BoltValue::Bool(true));
    }

    #[test]
    fn spec_hex_int_1() {
        let (val, _) = decode(&[0x01]).unwrap();
        assert_eq!(val, BoltValue::Int(1));
    }

    #[test]
    fn spec_hex_int_42() {
        let (val, _) = decode(&[0x2A]).unwrap();
        assert_eq!(val, BoltValue::Int(42));
    }

    #[test]
    fn spec_hex_float_1_1() {
        let (val, _) = decode(&[0xC1, 0x3F, 0xF1, 0x99, 0x99, 0x99, 0x99, 0x99, 0x9A]).unwrap();
        match val {
            BoltValue::Float(f) => assert!((f - 1.1).abs() < f64::EPSILON),
            _ => panic!("expected Float"),
        }
    }

    #[test]
    fn spec_hex_string_hello() {
        let (val, _) = decode(&[0x85, 0x68, 0x65, 0x6C, 0x6C, 0x6F]).unwrap();
        assert_eq!(val, BoltValue::String("hello".into()));
    }

    #[test]
    fn spec_hex_list_1_2_3() {
        let (val, _) = decode(&[0x93, 0x01, 0x02, 0x03]).unwrap();
        assert_eq!(
            val,
            BoltValue::List(vec![
                BoltValue::Int(1),
                BoltValue::Int(2),
                BoltValue::Int(3),
            ])
        );
    }

    #[test]
    fn spec_hex_map_one_1() {
        let (val, _) = decode(&[0xA1, 0x83, 0x6F, 0x6E, 0x65, 0x01]).unwrap();
        assert_eq!(val, BoltValue::Map(vec![("one".into(), BoltValue::Int(1))]));
    }

    // ── Extra bytes after value are not consumed ────────────────────────

    #[test]
    fn decode_consumes_only_needed_bytes() {
        let data = [0x01, 0x02, 0x03]; // Int(1) followed by extra bytes
        let (val, n) = decode(&data).unwrap();
        assert_eq!(val, BoltValue::Int(1));
        assert_eq!(n, 1); // only 1 byte consumed
    }

    // ── Depth limit tests ──────────────────────────────────────────────

    /// Build `depth` levels of nested TinyList(1 element), with a Null at the
    /// innermost level: 0x91 0x91 ... 0x91 0xC0
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
    fn test_list_at_max_prealloc_boundary_succeeds() {
        let count = super::MAX_PREALLOC; // 4096

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
}
