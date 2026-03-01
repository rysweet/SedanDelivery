#![no_main]
use libfuzzer_sys::fuzz_target;

// Feed arbitrary bytes into the chunk framing decoder.
// chunk_decode_sync must never panic -- only return Ok or Err.
fuzz_target!(|data: &[u8]| {
    let _ = sedandelivery_bolt::message::codec::chunk_decode_sync(data);
});
