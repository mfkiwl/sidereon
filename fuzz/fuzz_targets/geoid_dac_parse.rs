#![no_main]

use libfuzzer_sys::fuzz_target;
use sidereon_core::geoid::GeoidGrid;

fuzz_target!(|data: &[u8]| {
    let _ = GeoidGrid::from_egm96_dac(data);
});
