#![no_main]

use libfuzzer_sys::fuzz_target;
use sidereon_core::rtcm::{self, LockTimeTracker, Message};

fuzz_target!(|data: &[u8]| {
    let stream = rtcm::decode_stream(data);
    assert!(stream.diagnostics.resync_bytes <= data.len());
    assert!(stream.diagnostics.skipped_frames.len() <= data.len());

    let mut tracker = LockTimeTracker::new();
    for message in &stream.messages {
        if let Message::Msm(msm) = message {
            let cells = tracker.observe(msm);
            assert_eq!(cells.len(), msm.signals.len());
        }
    }
});
