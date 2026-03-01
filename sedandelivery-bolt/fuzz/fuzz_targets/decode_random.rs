#![no_main]
use libfuzzer_sys::fuzz_target;

// Feed arbitrary bytes into the PackStream decoder.
// The decoder must never panic -- it should return Ok or Err for any input.
fuzz_target!(|data: &[u8]| {
    let _ = sedandelivery_bolt::packstream::decoder::decode(data);
});
