#![no_main]

use libfuzzer_sys::fuzz_target;
use sidereon_core::astro::time::model::{GnssWeekTow, TimeScale};
use sidereon_core::sbas::{sbas_prn_to_sat, SbasBlock, SbasCorrectionStore, SbasWireForm};

const BODY_LEN: usize = 29;
const FRAMED_LEN: usize = 32;
const PREAMBLES: [u8; 3] = [0x53, 0x9A, 0xC6];

fn body_from_fuzz(data: &[u8]) -> [u8; BODY_LEN] {
    let mut body = [0u8; BODY_LEN];
    for (dst, src) in body.iter_mut().zip(data.iter()) {
        *dst = *src;
    }
    if let Some(&selector) = data.first() {
        body[0] = PREAMBLES[usize::from(selector) % PREAMBLES.len()];
    }
    if let Some(&message_type) = data.get(1) {
        body[1] = ((message_type & 0x3f) << 2) | (body[1] & 0x03);
    }
    body
}

fn framed_from_fuzz(data: &[u8]) -> [u8; FRAMED_LEN] {
    let mut framed = [0u8; FRAMED_LEN];
    for (dst, src) in framed.iter_mut().zip(data.iter()) {
        *dst = *src;
    }
    framed
}

fuzz_target!(|data: &[u8]| {
    let framed = framed_from_fuzz(data);
    let _ = SbasBlock::decode(&framed, SbasWireForm::Framed250);

    let body = body_from_fuzz(data);
    let Ok(decoded) = SbasBlock::decode(&body, SbasWireForm::Body226) else {
        return;
    };

    let geo = sbas_prn_to_sat(120).expect("valid SBAS source");
    let epoch = GnssWeekTow::new(TimeScale::Gpst, 2400, 0.0).expect("valid epoch");
    let mut store = SbasCorrectionStore::new();
    let _ = store.ingest(&decoded.message, geo, epoch);
});
