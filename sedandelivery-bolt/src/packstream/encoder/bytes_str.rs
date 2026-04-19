//! PackStream encoding for String and Bytes values.

pub(super) fn encode_string(s: &str, buf: &mut Vec<u8>) {
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

pub(super) fn encode_bytes(data: &[u8], buf: &mut Vec<u8>) {
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
