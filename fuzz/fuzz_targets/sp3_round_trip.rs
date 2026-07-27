#![no_main]

use libfuzzer_sys::fuzz_target;
use sidereon_core::ephemeris::Sp3;

fuzz_target!(|data: &[u8]| {
    let Ok(original) = Sp3::parse(data) else {
        return;
    };

    let encoded = original.to_sp3_string();
    let reparsed = Sp3::parse(encoded.as_bytes()).expect("encoded SP3 must reparse");

    // Serialization is idempotent for every product, including one carrying a
    // non-finite header value.
    assert_eq!(reparsed.to_sp3_string(), encoded);

    // `skipped_records` counts entries the input text carried but the product
    // cannot represent - an extended GLONASS slot such as `R28` beyond the
    // engine's PRN cap. Those are deliberately dropped instead of aborting the
    // parse (see `Sp3::skipped_records`), and nothing of them survives into the
    // product, so serialization has nothing to re-emit and a faithful re-encode
    // always reports zero. Asserting zero is stricter than comparing the two
    // counts: the writer must never emit a record that re-parses as
    // unrepresentable.
    let mut expected = original;
    expected.skipped_records = 0;

    // `Sp3` compares its f64 header fields, so a product whose seconds-of-week
    // or epoch interval is non-finite - which `Sp3::parse` deliberately keeps
    // for `validate_exact_sp3` to reject as a typed integrity failure - is not
    // equal to itself, and structural equality says nothing about it. The
    // byte-idempotence assertion above still covers those products. Everything
    // else must match exactly, including the private interpolation nodes.
    let comparable = expected == expected.clone();
    if comparable {
        assert_eq!(reparsed, expected);
    }
});
