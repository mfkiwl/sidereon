#![no_main]

use libfuzzer_sys::fuzz_target;
use sidereon_core::observation_qc::observation_qc;
use sidereon_core::rinex::observations::RinexObs;
use sidereon_core::rinex::qc::{lint_obs, repair_obs, RepairOptions};

const MAX_INPUT_LEN: usize = 1 << 20;

fn repair_options() -> RepairOptions {
    RepairOptions {
        set_interval: true,
        set_time_of_last_obs: true,
        set_obs_counts: true,
        drop_empty_records: true,
        drop_unsupported: true,
        ..RepairOptions::default()
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_LEN {
        return;
    }

    let text = String::from_utf8_lossy(data);
    let Ok(obs) = RinexObs::parse(&text) else {
        return;
    };

    let _ = observation_qc(&obs);
    let _ = lint_obs(&obs);

    let options = repair_options();
    let repair = repair_obs(&obs, &options);
    let repaired_text = repair.repaired.to_rinex_string();
    let reparsed = RinexObs::parse(&repaired_text).expect("repaired OBS must reparse");

    let _ = observation_qc(&reparsed);
    let _ = lint_obs(&reparsed);

    let repeated = repair_obs(&reparsed, &options);
    let repeated_text = repeated.repaired.to_rinex_string();
    assert_eq!(repeated_text.as_bytes(), repaired_text.as_bytes());
});
