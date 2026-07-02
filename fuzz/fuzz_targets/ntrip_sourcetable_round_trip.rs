#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Ok(table) = sidereon_core::ntrip::parse_sourcetable(&text) {
        let rendered = table.to_text();
        let reparsed = sidereon_core::ntrip::parse_sourcetable(&rendered).unwrap();
        assert_eq!(reparsed, table);
    }
});
