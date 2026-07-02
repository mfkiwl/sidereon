use sidereon::ephemeris::{sample, EphemerisSampleStatus, Sp3};
use sidereon::{GnssSatelliteId, GnssSystem};

const DEGENERATE_SP3: &[u8] =
    include_bytes!("../../sidereon-core/tests/fixtures/sp3/degenerate_coincident_5sat.sp3");

#[test]
fn facade_reexports_ephemeris_sampler() {
    let sp3 = Sp3::parse(DEGENERATE_SP3).expect("parse SP3 fixture");
    let sat = GnssSatelliteId::new(GnssSystem::Gps, 1).expect("valid G01");
    let epochs = sp3.epochs_j2000_seconds();

    let rows = sample(&sp3, &[sat], epochs[0], epochs[1], epochs[1] - epochs[0])
        .expect("sample through facade re-export");

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].sat, sat);
    assert_eq!(rows[0].status, EphemerisSampleStatus::Valid);
    assert!(rows[0].position_ecef_m.is_some());
    assert_eq!(rows[0].clock_s, Some(0.0));
}
