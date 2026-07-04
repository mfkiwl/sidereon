#![no_main]

use libfuzzer_sys::fuzz_target;
use sidereon_core::astro::tdm;

// Round-trip class: a parsed TDM must re-encode to text that reparses to an
// equal value, and the canonical encoding must then be byte-stable.
fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);

    if let Ok(original) = tdm::parse_kvn(&text) {
        let Ok(encoded) = tdm::encode_kvn(&original) else {
            return;
        };
        let reparsed = tdm::parse_kvn(&encoded).expect("encoded TDM KVN must reparse");
        assert_eq!(reparsed, original);
        assert_eq!(
            tdm::encode_kvn(&reparsed).expect("reparsed TDM KVN must encode"),
            encoded
        );
    }
});
