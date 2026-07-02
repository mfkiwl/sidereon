#![no_main]

use libfuzzer_sys::fuzz_target;
use sidereon_core::nmea::NmeaAccumulator;

fuzz_target!(|data: &[u8]| {
    let mut accumulator = NmeaAccumulator::new();
    for chunk in data.chunks(7) {
        let _ = accumulator.push_bytes(chunk);
    }
    let _ = accumulator.finish();
});
