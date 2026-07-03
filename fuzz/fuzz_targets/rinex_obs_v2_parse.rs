#![no_main]

use libfuzzer_sys::fuzz_target;
use sidereon_core::rinex::observations::RinexObs;

fn header_line(body: &str, label: &str) -> String {
    format!("{body:<60}{label}\n")
}

fuzz_target!(|data: &[u8]| {
    let body = String::from_utf8_lossy(data);
    let mut text = String::new();
    text.push_str(&header_line(
        "     2.11           OBSERVATION DATA    G (GPS)",
        "RINEX VERSION / TYPE",
    ));
    text.push_str(&header_line("     6    L1    L2    C1    P1    S1    S2", "# / TYPES OF OBSERV"));
    text.push_str(&header_line("", "END OF HEADER"));
    text.push_str(&body);

    let Ok(obs) = RinexObs::parse(&text) else {
        return;
    };

    let bound = data.len().saturating_add(1);
    assert!(obs.epochs().len() <= bound);
    let retained_satellites: usize = obs.epochs().iter().map(|epoch| epoch.sats.len()).sum();
    assert!(retained_satellites <= bound);
});
