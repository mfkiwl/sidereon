#![no_main]

use libfuzzer_sys::fuzz_target;
use sidereon_core::ntrip::{NtripClientMachine, NtripConfig};

fuzz_target!(|data: &[u8]| {
    let mut machine = NtripClientMachine::new(NtripConfig::default());
    let _ = machine.connection_request();
    let _ = machine.push(data);
});
