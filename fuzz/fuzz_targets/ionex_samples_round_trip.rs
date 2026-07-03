#![no_main]

use libfuzzer_sys::fuzz_target;
use sidereon_core::atmosphere::Ionex;

fuzz_target!(|data: &[u8]| {
    let Ok(original) = Ionex::parse(data) else {
        return;
    };
    if original.skipped_records() != 0 {
        return;
    }

    let rebuilt =
        Ionex::from_samples(original.tec_grid_samples()).expect("valid parsed IONEX samples");
    assert_eq!(rebuilt, original);
});
