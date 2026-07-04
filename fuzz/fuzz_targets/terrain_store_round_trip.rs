#![no_main]

use libfuzzer_sys::fuzz_target;
use sidereon_core::terrain_store::MmapTerrain;

fuzz_target!(|data: &[u8]| {
    if let Ok(store) = MmapTerrain::from_bytes(data) {
        let round_trip = store.to_bytes();
        assert_eq!(round_trip, data);
        MmapTerrain::from_bytes(&round_trip).expect("round-trip terrain store parses");
    }
});
