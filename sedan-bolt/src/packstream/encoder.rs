use super::value::BoltValue;

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

fn encode_int(n: i64, buf: &mut Vec<u8>) {
    if (-16..=127).contains(&n) {
        // TinyInt: the value itself is the single marker byte.
        buf.push(n as u8);
    } else if (-128..=-17).contains(&n) {
        // Int8
        buf.push(0xC8);
        buf.push(n as i8 as u8);
    } else if (-32_768..=32_767).contains(&n) {
        // Int16
        buf.push(0xC9);
        buf.extend_from_slice(&(n as i16).to_be_bytes());
    } else if (-2_147_483_648..=2_147_483_647).contains(&n) {
        // Int32
        buf.push(0xCA);
        buf.extend_from_slice(&(n as i32).to_be_bytes());
    } else {
        // Int64
        buf.push(0xCB);
        buf.extend_from_slice(&n.to_be_bytes());
    }
}

fn encode_float(f: f64, buf: &mut Vec<u8>) {
    buf.push(0xC1);
    buf.extend_from_slice(&f.to_be_bytes());
}

fn encode_string(s: &str, buf: &mut Vec<u8>) {
    let bytes = s.as_bytes();
    let len = bytes.len();
    if len <= 15 {
        buf.push(0x80 | len as u8);
    } else if len <= 255 {
        buf.push(0xD0);
        buf.push(len as u8);
    } else if len <= 65_535 {
        buf.push(0xD1);
        buf.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        buf.push(0xD2);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
    }
    buf.extend_from_slice(bytes);
}

fn encode_bytes(data: &[u8], buf: &mut Vec<u8>) {
    let len = data.len();
    if len <= 255 {
        buf.push(0xCC);
        buf.push(len as u8);
    } else if len <= 65_535 {
        buf.push(0xCD);
        buf.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        buf.push(0xCE);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
    }
    buf.extend_from_slice(data);
}

fn encode_list(items: &[BoltValue], buf: &mut Vec<u8>) {
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

fn encode_map(entries: &[(String, BoltValue)], buf: &mut Vec<u8>) {
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

fn encode_structure(tag: u8, fields: &[BoltValue], buf: &mut Vec<u8>) {
    let nfields = fields.len().min(15);
    buf.push(0xB0 | nfields as u8);
    buf.push(tag);
    for field in &fields[..nfields] {
        encode(field, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(v: &BoltValue) -> Vec<u8> {
        let mut buf = Vec::new();
        encode(v, &mut buf);
        buf
    }

    // ── Null ────────────────────────────────────────────────────────────

    #[test]
    fn encode_null() {
        assert_eq!(enc(&BoltValue::Null), vec![0xC0]);
    }

    // ── Bool ────────────────────────────────────────────────────────────

    #[test]
    fn encode_false() {
        assert_eq!(enc(&BoltValue::Bool(false)), vec![0xC2]);
    }

    #[test]
    fn encode_true() {
        assert_eq!(enc(&BoltValue::Bool(true)), vec![0xC3]);
    }

    // ── Int: TinyInt ────────────────────────────────────────────────────

    #[test]
    fn encode_int_zero() {
        assert_eq!(enc(&BoltValue::Int(0)), vec![0x00]);
    }

    #[test]
    fn encode_int_one() {
        assert_eq!(enc(&BoltValue::Int(1)), vec![0x01]);
    }

    #[test]
    fn encode_int_127() {
        assert_eq!(enc(&BoltValue::Int(127)), vec![0x7F]);
    }

    #[test]
    fn encode_int_neg1() {
        // -1 as u8 = 0xFF, within TinyInt range
        assert_eq!(enc(&BoltValue::Int(-1)), vec![0xFF]);
    }

    #[test]
    fn encode_int_neg16() {
        // -16 as u8 = 0xF0, lower bound of TinyInt
        assert_eq!(enc(&BoltValue::Int(-16)), vec![0xF0]);
    }

    // ── Int: Int8 ───────────────────────────────────────────────────────

    #[test]
    fn encode_int_neg17() {
        // -17 = 0xEF as i8
        assert_eq!(enc(&BoltValue::Int(-17)), vec![0xC8, 0xEF]);
    }

    #[test]
    fn encode_int_neg128() {
        // -128 = 0x80 as i8
        assert_eq!(enc(&BoltValue::Int(-128)), vec![0xC8, 0x80]);
    }

    // ── Int: Int16 ──────────────────────────────────────────────────────

    #[test]
    fn encode_int_128() {
        // 128 does not fit TinyInt (max 127), not negative so not Int8 → Int16
        assert_eq!(enc(&BoltValue::Int(128)), vec![0xC9, 0x00, 0x80]);
    }

    #[test]
    fn encode_int_neg129() {
        // -129 does not fit Int8 (min -128) → Int16
        assert_eq!(enc(&BoltValue::Int(-129)), vec![0xC9, 0xFF, 0x7F]);
    }

    #[test]
    fn encode_int_32767() {
        assert_eq!(enc(&BoltValue::Int(32767)), vec![0xC9, 0x7F, 0xFF]);
    }

    #[test]
    fn encode_int_neg32768() {
        assert_eq!(enc(&BoltValue::Int(-32768)), vec![0xC9, 0x80, 0x00]);
    }

    // ── Int: Int32 ──────────────────────────────────────────────────────

    #[test]
    fn encode_int_32768() {
        assert_eq!(
            enc(&BoltValue::Int(32768)),
            vec![0xCA, 0x00, 0x00, 0x80, 0x00]
        );
    }

    #[test]
    fn encode_int_neg32769() {
        // -32769 does not fit Int16
        let expected = (-32769_i32).to_be_bytes();
        let mut want = vec![0xCA];
        want.extend_from_slice(&expected);
        assert_eq!(enc(&BoltValue::Int(-32769)), want);
    }

    #[test]
    fn encode_int_i32_max() {
        assert_eq!(
            enc(&BoltValue::Int(i32::MAX as i64)),
            vec![0xCA, 0x7F, 0xFF, 0xFF, 0xFF]
        );
    }

    #[test]
    fn encode_int_i32_min() {
        assert_eq!(
            enc(&BoltValue::Int(i32::MIN as i64)),
            vec![0xCA, 0x80, 0x00, 0x00, 0x00]
        );
    }

    // ── Int: Int64 ──────────────────────────────────────────────────────

    #[test]
    fn encode_int_i64_max() {
        let mut want = vec![0xCB];
        want.extend_from_slice(&i64::MAX.to_be_bytes());
        assert_eq!(enc(&BoltValue::Int(i64::MAX)), want);
    }

    #[test]
    fn encode_int_i64_min() {
        let mut want = vec![0xCB];
        want.extend_from_slice(&i64::MIN.to_be_bytes());
        assert_eq!(enc(&BoltValue::Int(i64::MIN)), want);
    }

    #[test]
    fn encode_int_above_i32_max() {
        let n = i32::MAX as i64 + 1;
        let mut want = vec![0xCB];
        want.extend_from_slice(&n.to_be_bytes());
        assert_eq!(enc(&BoltValue::Int(n)), want);
    }

    // ── Float ───────────────────────────────────────────────────────────

    #[test]
    fn encode_float_positive() {
        let result = enc(&BoltValue::Float(1.1));
        assert_eq!(result.len(), 9);
        assert_eq!(result[0], 0xC1);
        let f = f64::from_be_bytes(result[1..9].try_into().unwrap());
        assert!((f - 1.1).abs() < f64::EPSILON);
    }

    #[test]
    fn encode_float_zero() {
        let result = enc(&BoltValue::Float(0.0));
        assert_eq!(result[0], 0xC1);
        assert_eq!(result.len(), 9);
    }

    // ── String ──────────────────────────────────────────────────────────

    #[test]
    fn encode_empty_string() {
        // TinyString with len 0: marker = 0x80
        assert_eq!(enc(&BoltValue::String(String::new())), vec![0x80]);
    }

    #[test]
    fn encode_tiny_string() {
        let s = "A";
        let result = enc(&BoltValue::String(s.into()));
        assert_eq!(result, vec![0x81, b'A']);
    }

    #[test]
    fn encode_tiny_string_max() {
        let s = "abcdefghijklmno"; // exactly 15 bytes
        let result = enc(&BoltValue::String(s.into()));
        assert_eq!(result[0], 0x8F);
        assert_eq!(&result[1..], s.as_bytes());
    }

    #[test]
    fn encode_string8() {
        // 16 bytes → String8
        let s: String = "a".repeat(16);
        let result = enc(&BoltValue::String(s.clone()));
        assert_eq!(result[0], 0xD0);
        assert_eq!(result[1], 16);
        assert_eq!(&result[2..], s.as_bytes());
    }

    #[test]
    fn encode_string8_max() {
        let s: String = "x".repeat(255);
        let result = enc(&BoltValue::String(s.clone()));
        assert_eq!(result[0], 0xD0);
        assert_eq!(result[1], 255);
        assert_eq!(result.len(), 2 + 255);
    }

    #[test]
    fn encode_string16() {
        let s: String = "y".repeat(256);
        let result = enc(&BoltValue::String(s.clone()));
        assert_eq!(result[0], 0xD1);
        let len = u16::from_be_bytes([result[1], result[2]]);
        assert_eq!(len, 256);
        assert_eq!(result.len(), 3 + 256);
    }

    #[test]
    fn encode_string32() {
        let s: String = "z".repeat(65_536);
        let result = enc(&BoltValue::String(s.clone()));
        assert_eq!(result[0], 0xD2);
        let len = u32::from_be_bytes([result[1], result[2], result[3], result[4]]);
        assert_eq!(len, 65_536);
        assert_eq!(result.len(), 5 + 65_536);
    }

    // ── Bytes ───────────────────────────────────────────────────────────

    #[test]
    fn encode_empty_bytes() {
        assert_eq!(enc(&BoltValue::Bytes(vec![])), vec![0xCC, 0x00]);
    }

    #[test]
    fn encode_bytes8() {
        let data = vec![0xAB; 10];
        let result = enc(&BoltValue::Bytes(data.clone()));
        assert_eq!(result[0], 0xCC);
        assert_eq!(result[1], 10);
        assert_eq!(&result[2..], &data[..]);
    }

    #[test]
    fn encode_bytes16() {
        let data = vec![0x01; 256];
        let result = enc(&BoltValue::Bytes(data.clone()));
        assert_eq!(result[0], 0xCD);
        let len = u16::from_be_bytes([result[1], result[2]]);
        assert_eq!(len, 256);
    }

    // ── List ────────────────────────────────────────────────────────────

    #[test]
    fn encode_empty_list() {
        assert_eq!(enc(&BoltValue::List(vec![])), vec![0x90]);
    }

    #[test]
    fn encode_tiny_list() {
        let list = vec![BoltValue::Int(1), BoltValue::Int(2), BoltValue::Int(3)];
        let result = enc(&BoltValue::List(list));
        // 0x93, then 0x01, 0x02, 0x03
        assert_eq!(result, vec![0x93, 0x01, 0x02, 0x03]);
    }

    #[test]
    fn encode_list8() {
        // 16 items → List8
        let list: Vec<BoltValue> = (0..16).map(|i| BoltValue::Int(i)).collect();
        let result = enc(&BoltValue::List(list));
        assert_eq!(result[0], 0xD4);
        assert_eq!(result[1], 16);
    }

    // ── Map ─────────────────────────────────────────────────────────────

    #[test]
    fn encode_empty_map() {
        assert_eq!(enc(&BoltValue::Map(vec![])), vec![0xA0]);
    }

    #[test]
    fn encode_tiny_map() {
        let map = vec![("one".into(), BoltValue::Int(1))];
        let result = enc(&BoltValue::Map(map));
        // 0xA1, then key "one" (0x83 + b"one"), then value 1 (0x01)
        assert_eq!(
            result,
            vec![0xA1, 0x83, b'o', b'n', b'e', 0x01]
        );
    }

    // ── Structure ───────────────────────────────────────────────────────

    #[test]
    fn encode_structure_no_fields() {
        let s = BoltValue::Structure {
            tag: 0x01,
            fields: vec![],
        };
        assert_eq!(enc(&s), vec![0xB0, 0x01]);
    }

    #[test]
    fn encode_structure_with_fields() {
        let s = BoltValue::Structure {
            tag: 0x71,
            fields: vec![BoltValue::Int(42)],
        };
        let result = enc(&s);
        assert_eq!(result, vec![0xB1, 0x71, 0x2A]);
    }

    // ── Roundtrip sanity (uses decoder in decoder.rs tests) ─────────────

    #[test]
    fn encode_nested_list_and_map() {
        let val = BoltValue::List(vec![
            BoltValue::Map(vec![
                ("a".into(), BoltValue::Int(1)),
                ("b".into(), BoltValue::Bool(true)),
            ]),
            BoltValue::Null,
        ]);
        let result = enc(&val);
        // Should start with 0x92 (list of 2)
        assert_eq!(result[0], 0x92);
        // Second-to-last byte before Null should be Null marker
        assert_eq!(*result.last().unwrap(), 0xC0);
    }

    // ── Spec hex examples ───────────────────────────────────────────────

    #[test]
    fn spec_example_int_1() {
        // PackStream spec: integer 1 → 0x01
        assert_eq!(enc(&BoltValue::Int(1)), vec![0x01]);
    }

    #[test]
    fn spec_example_int_42() {
        // PackStream spec: integer 42 → 0x2A
        assert_eq!(enc(&BoltValue::Int(42)), vec![0x2A]);
    }

    #[test]
    fn spec_example_int_neg7() {
        // -7 as TinyInt: 0xF9
        assert_eq!(enc(&BoltValue::Int(-7)), vec![0xF9]);
    }

    #[test]
    fn spec_example_int_neg16() {
        // -16 as TinyInt: 0xF0
        assert_eq!(enc(&BoltValue::Int(-16)), vec![0xF0]);
    }

    #[test]
    fn spec_example_string_hello() {
        // "hello" → 0x85 68 65 6C 6C 6F
        assert_eq!(
            enc(&BoltValue::String("hello".into())),
            vec![0x85, 0x68, 0x65, 0x6C, 0x6C, 0x6F]
        );
    }

    #[test]
    fn spec_example_list_1_2_3() {
        // [1,2,3] → 0x93 01 02 03
        let v = BoltValue::List(vec![
            BoltValue::Int(1),
            BoltValue::Int(2),
            BoltValue::Int(3),
        ]);
        assert_eq!(enc(&v), vec![0x93, 0x01, 0x02, 0x03]);
    }

    #[test]
    fn spec_example_float_1_1() {
        // 1.1 → 0xC1 3F F1 99 99 99 99 99 9A
        assert_eq!(
            enc(&BoltValue::Float(1.1)),
            vec![0xC1, 0x3F, 0xF1, 0x99, 0x99, 0x99, 0x99, 0x99, 0x9A]
        );
    }

    #[test]
    fn spec_example_map_one_1() {
        // {"one": 1} → A1 83 6F 6E 65 01
        let v = BoltValue::Map(vec![("one".into(), BoltValue::Int(1))]);
        assert_eq!(
            enc(&v),
            vec![0xA1, 0x83, 0x6F, 0x6E, 0x65, 0x01]
        );
    }
}
