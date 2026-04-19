//! PackStream encoding for scalar numeric values (Int, Float).

pub(super) fn encode_int(n: i64, buf: &mut Vec<u8>) {
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

pub(super) fn encode_float(f: f64, buf: &mut Vec<u8>) {
    buf.push(0xC1);
    buf.extend_from_slice(&f.to_be_bytes());
}
