use std::path::PathBuf;

use sidereon_core::constants::{GPS_EPOCH_TO_J2000_S, SECONDS_PER_WEEK};
use sidereon_core::ephemeris::{
    sample, BroadcastEphemeris, EphemerisSampleStatus, ObservableEphemerisSource, Sp3,
};
use sidereon_core::{GnssSatelliteId, GnssSystem};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn load_sp3_fixture(name: &str) -> Sp3 {
    let bytes = std::fs::read(fixture_path(name)).expect("read SP3 fixture");
    Sp3::parse(&bytes).expect("parse SP3 fixture")
}

#[test]
fn sample_sp3_rows_match_single_epoch_evaluator() {
    let sp3 = load_sp3_fixture("sp3/IGS0OPSFIN_20261330000_03H_15M_ORB.SP3");
    let sat = *sp3
        .satellites()
        .iter()
        .find(|sat| sat.system == GnssSystem::Gps)
        .expect("GPS satellite in fixture");
    let epochs = sp3.epochs_j2000_seconds();
    let start = epochs[0];
    let stop = epochs[1];
    let step = stop - start;

    let rows = sample(&sp3, &[sat], start, stop, step).expect("sample SP3");

    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row.sat, sat);
        assert_eq!(row.status, EphemerisSampleStatus::Valid);
        let expected = sp3
            .position_at_j2000_seconds(sat, row.epoch_j2000_s)
            .expect("single epoch SP3 state");
        assert_eq!(
            row.position_ecef_m.expect("sample position"),
            expected.position.as_array()
        );
        assert_eq!(row.clock_s, expected.clock_s);
    }
}

#[test]
fn sample_broadcast_rows_match_single_epoch_evaluator() {
    let text = std::fs::read_to_string(fixture_path("nav/ESBC00DNK_R_20201770000_01D_MN.rnx"))
        .expect("read NAV fixture");
    let broadcast = BroadcastEphemeris::from_nav(&text).expect("parse NAV fixture");
    let record = broadcast
        .records()
        .iter()
        .find(|record| record.satellite_id.system == GnssSystem::Gps)
        .expect("GPS broadcast record");
    let sat = record.satellite_id;
    let start =
        f64::from(record.toe.week) * SECONDS_PER_WEEK + record.toe.tow_s - GPS_EPOCH_TO_J2000_S;
    let stop = start + 60.0;

    let rows = sample(&broadcast, &[sat], start, stop, 60.0).expect("sample broadcast");

    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row.sat, sat);
        assert_eq!(row.status, EphemerisSampleStatus::Valid);
        let expected = broadcast
            .observable_state_at_j2000_s(sat, row.epoch_j2000_s)
            .expect("single epoch broadcast state");
        assert_eq!(
            row.position_ecef_m.expect("sample position"),
            expected.position_ecef_m
        );
        assert_eq!(row.clock_s, expected.clock_s);
        assert!(row.clock_s.is_some(), "broadcast clock should be present");
    }
}

#[test]
fn sample_sp3_gap_is_a_gap_row_not_an_error() {
    let sp3 = load_sp3_fixture("sp3/GAP_G01_20201760000_15M.sp3");
    let sat = GnssSatelliteId::new(GnssSystem::Gps, 1).expect("valid G01");
    let gap_epoch_j2000_s = 646_260_300.0;

    let rows = sample(&sp3, &[sat], gap_epoch_j2000_s, gap_epoch_j2000_s, 900.0)
        .expect("gap should not fail sampling");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].sat, sat);
    assert_eq!(rows[0].epoch_j2000_s, gap_epoch_j2000_s);
    assert_eq!(rows[0].status, EphemerisSampleStatus::Gap);
    assert!(rows[0].is_gap());
    assert_eq!(rows[0].position_ecef_m, None);
    assert_eq!(rows[0].clock_s, None);
}
