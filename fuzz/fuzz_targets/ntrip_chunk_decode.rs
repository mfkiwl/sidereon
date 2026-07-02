#![no_main]

use libfuzzer_sys::fuzz_target;
use sidereon_core::ntrip::ChunkedDecoder;

fuzz_target!(|data: &[u8]| {
    let mut whole = ChunkedDecoder::new();
    let whole_out = whole.push(data);

    let mut split = ChunkedDecoder::new();
    let mut split_bytes = Vec::new();
    let mut split_error = false;
    for byte in data {
        match split.push(&[*byte]) {
            Ok(bytes) => split_bytes.extend(bytes),
            Err(_) => {
                split_error = true;
                break;
            }
        }
    }
    if let Ok(whole_bytes) = whole_out {
        if !split_error {
            assert_eq!(whole_bytes, split_bytes);
        }
    }
});
