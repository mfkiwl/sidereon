#![no_main]

use libfuzzer_sys::fuzz_target;

const MAX_INPUT_LEN: usize = 1 << 20;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_LEN {
        return;
    }

    let _ = sidereon_core::astro::space_weather::parse(data);
});
