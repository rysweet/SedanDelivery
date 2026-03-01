#![no_main]
use libfuzzer_sys::fuzz_target;
use arbitrary::{Arbitrary, Unstructured};
use sedan_bolt::packstream::value::BoltValue;

/// Maximum nesting depth for generated BoltValues.
/// Keeps fuzzer memory usage reasonable and avoids stack overflow.
const MAX_DEPTH: u8 = 3;

/// Maximum number of elements in a generated List or Map.
const MAX_COLLECTION_LEN: usize = 8;

/// Maximum length of generated strings and byte arrays.
const MAX_BYTES_LEN: usize = 64;

/// Maximum number of structure fields (PackStream TinyStruct limit is 15).
const MAX_STRUCT_FIELDS: usize = 15;

/// Wrapper that implements Arbitrary with depth-limited BoltValue generation.
struct ArbitraryBoltValue(BoltValue);

impl<'a> Arbitrary<'a> for ArbitraryBoltValue {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(ArbitraryBoltValue(gen_bolt_value(u, 0)?))
    }
}

fn gen_bolt_value(u: &mut Unstructured, depth: u8) -> arbitrary::Result<BoltValue> {
    // At max depth, only generate leaf values (no recursion).
    let max_variant = if depth >= MAX_DEPTH { 5 } else { 9 };
    let variant: u8 = u.int_in_range(0..=max_variant)?;

    match variant {
        0 => Ok(BoltValue::Null),
        1 => Ok(BoltValue::Bool(u.arbitrary()?)),
        2 => Ok(BoltValue::Int(u.arbitrary()?)),
        3 => {
            // Generate a float, but avoid NaN so roundtrip equality works.
            let f: f64 = u.arbitrary()?;
            if f.is_nan() {
                Ok(BoltValue::Float(0.0))
            } else {
                Ok(BoltValue::Float(f))
            }
        }
        4 => {
            let len = u.int_in_range(0..=MAX_BYTES_LEN)?;
            let data: Vec<u8> = (0..len).map(|_| u.arbitrary()).collect::<arbitrary::Result<_>>()?;
            Ok(BoltValue::Bytes(data))
        }
        5 => {
            // Generate a valid UTF-8 string of bounded length.
            let len = u.int_in_range(0..=MAX_BYTES_LEN)?;
            let s: String = (0..len)
                .map(|_| u.arbitrary::<char>())
                .collect::<arbitrary::Result<_>>()?;
            Ok(BoltValue::String(s))
        }
        6 => {
            // List -- recurse with incremented depth.
            let count = u.int_in_range(0..=MAX_COLLECTION_LEN)?;
            let items: Vec<BoltValue> = (0..count)
                .map(|_| gen_bolt_value(u, depth + 1))
                .collect::<arbitrary::Result<_>>()?;
            Ok(BoltValue::List(items))
        }
        7 => {
            // Map -- keys are strings, values recurse.
            let count = u.int_in_range(0..=MAX_COLLECTION_LEN)?;
            let entries: Vec<(String, BoltValue)> = (0..count)
                .map(|_| {
                    let klen = u.int_in_range(0..=16)?;
                    let key: String = (0..klen)
                        .map(|_| u.arbitrary::<char>())
                        .collect::<arbitrary::Result<_>>()?;
                    let val = gen_bolt_value(u, depth + 1)?;
                    Ok((key, val))
                })
                .collect::<arbitrary::Result<_>>()?;
            Ok(BoltValue::Map(entries))
        }
        8 => {
            // Structure -- tag byte + bounded field count.
            let tag: u8 = u.arbitrary()?;
            let nfields = u.int_in_range(0..=MAX_STRUCT_FIELDS)?;
            let fields: Vec<BoltValue> = (0..nfields)
                .map(|_| gen_bolt_value(u, depth + 1))
                .collect::<arbitrary::Result<_>>()?;
            Ok(BoltValue::Structure { tag, fields })
        }
        _ => {
            // Fallback (should not be reached due to int_in_range bounds).
            Ok(BoltValue::Null)
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let Ok(wrapper) = ArbitraryBoltValue::arbitrary(&mut u) else {
        return;
    };
    let val = wrapper.0;

    // Encode the value.
    let mut buf = Vec::new();
    sedan_bolt::packstream::encoder::encode(&val, &mut buf);

    // Decode the encoded bytes.
    match sedan_bolt::packstream::decoder::decode(&buf) {
        Ok((decoded, consumed)) => {
            assert_eq!(consumed, buf.len(), "decoder did not consume all encoded bytes");
            assert_eq!(decoded, val, "roundtrip mismatch");
        }
        Err(e) => {
            // If encode produced bytes that decode rejects, that is a bug.
            panic!("decode failed on encoder output: {e}");
        }
    }
});
