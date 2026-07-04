use std::path::PathBuf;

use sidereon_core::constants::{GPS_EPOCH_TO_J2000_S, SECONDS_PER_WEEK};
use sidereon_core::ephemeris::{
    observable_states_at_j2000_s, observable_states_at_shared_j2000_s, BroadcastEphemeris,
    ObservableEphemerisSource, ObservableStateBatch, ObservableStateElementStatus,
    PreciseEphemerisInterpolant, Sp3, OBSERVABLE_STATE_MISSING_POSITION_ECEF_M,
};
use sidereon_core::observables::{ObservableState, ObservablesError};
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

fn load_broadcast_fixture(name: &str) -> BroadcastEphemeris {
    let text = std::fs::read_to_string(fixture_path(name)).expect("read NAV fixture");
    BroadcastEphemeris::from_nav(&text).expect("parse NAV fixture")
}

fn sbas(prn: u8) -> GnssSatelliteId {
    GnssSatelliteId::new(GnssSystem::Sbas, prn).expect("valid SBAS satellite")
}

fn toe_j2000_s(record: &sidereon_core::ephemeris::BroadcastRecord) -> f64 {
    f64::from(record.toe.week) * SECONDS_PER_WEEK + record.toe.tow_s - GPS_EPOCH_TO_J2000_S
}

fn assert_state_bits_eq(actual: ObservableState, expected: ObservableState) {
    assert_eq!(
        actual.position_ecef_m.map(f64::to_bits),
        expected.position_ecef_m.map(f64::to_bits)
    );
    assert_eq!(
        actual.clock_s.map(f64::to_bits),
        expected.clock_s.map(f64::to_bits)
    );
}

fn assert_element_matches_scalar(
    batch: &ObservableStateBatch,
    index: usize,
    expected: Result<ObservableState, ObservablesError>,
) {
    match expected {
        Ok(state) => {
            assert_eq!(batch.element_results[index], Ok(()));
            assert_state_bits_eq(
                ObservableState {
                    position_ecef_m: batch.positions_ecef_m[index],
                    clock_s: batch.clocks_s[index],
                },
                state,
            );
            assert_state_bits_eq(batch.element(index).unwrap().unwrap(), state);
            assert_eq!(
                batch.element_status(index),
                Some(ObservableStateElementStatus::Valid)
            );
        }
        Err(error) => {
            assert_eq!(batch.element_results[index], Err(error.clone()));
            assert_eq!(
                batch.positions_ecef_m[index].map(f64::to_bits),
                OBSERVABLE_STATE_MISSING_POSITION_ECEF_M.map(f64::to_bits)
            );
            assert_eq!(batch.clocks_s[index], None);
            assert_eq!(batch.element(index).unwrap().unwrap_err(), &error);
        }
    }
}

fn assert_batch_matches_scalar(
    source: &dyn ObservableEphemerisSource,
    satellites: &[GnssSatelliteId],
    epochs_j2000_s: &[f64],
    batch: &ObservableStateBatch,
) {
    assert_eq!(batch.len(), satellites.len());
    assert_eq!(batch.positions_ecef_m.len(), satellites.len());
    assert_eq!(batch.clocks_s.len(), satellites.len());
    assert_eq!(batch.element_results.len(), satellites.len());

    for (index, (&sat, &epoch_j2000_s)) in satellites.iter().zip(epochs_j2000_s.iter()).enumerate()
    {
        let expected = source.observable_state_at_j2000_s(sat, epoch_j2000_s);
        assert_element_matches_scalar(batch, index, expected);
    }
}

fn assert_batch_bits_eq(left: &ObservableStateBatch, right: &ObservableStateBatch) {
    assert_eq!(
        left.positions_ecef_m
            .iter()
            .map(|position| position.map(f64::to_bits))
            .collect::<Vec<_>>(),
        right
            .positions_ecef_m
            .iter()
            .map(|position| position.map(f64::to_bits))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        left.clocks_s
            .iter()
            .map(|clock| clock.map(f64::to_bits))
            .collect::<Vec<_>>(),
        right
            .clocks_s
            .iter()
            .map(|clock| clock.map(f64::to_bits))
            .collect::<Vec<_>>()
    );
    assert_eq!(left.element_results, right.element_results);
}

#[test]
fn precise_and_broadcast_state_batches_match_scalar_loop() {
    let sp3 = load_sp3_fixture("sp3/IGS0OPSFIN_20261330000_03H_15M_ORB.SP3");
    let handle = PreciseEphemerisInterpolant::from_sp3(&sp3);
    let epochs = sp3.epochs_j2000_seconds();
    let nominal_step_s = epochs[1] - epochs[0];
    let shared_epoch = 0.5 * (epochs[1] + epochs[2]);
    let boundary_epoch = epochs[0] - nominal_step_s;
    let missing_precise = sbas(20);
    let precise_sats = [sp3.satellites()[0], sp3.satellites()[1], missing_precise];

    let direct_shared = observable_states_at_shared_j2000_s(&sp3, &precise_sats, shared_epoch);
    let shared_epochs = vec![shared_epoch; precise_sats.len()];
    assert_batch_matches_scalar(&sp3, &precise_sats, &shared_epochs, &direct_shared);
    assert_eq!(
        direct_shared.element_status(2),
        Some(ObservableStateElementStatus::Gap)
    );

    let handle_shared = handle.observable_states_at_shared_j2000_s(&precise_sats, shared_epoch);
    assert_batch_bits_eq(&direct_shared, &handle_shared);

    let precise_epoch_sats = [
        sp3.satellites()[0],
        sp3.satellites()[1],
        missing_precise,
        sp3.satellites()[0],
    ];
    let precise_epochs = [
        epochs[0],
        0.25 * epochs[1] + 0.75 * epochs[2],
        epochs[1],
        boundary_epoch,
    ];
    let direct_epochs = observable_states_at_j2000_s(&sp3, &precise_epoch_sats, &precise_epochs)
        .expect("matching precise batch lengths");
    assert_batch_matches_scalar(&sp3, &precise_epoch_sats, &precise_epochs, &direct_epochs);

    let handle_epochs = handle
        .observable_states_at_j2000_s(&precise_epoch_sats, &precise_epochs)
        .expect("matching handle batch lengths");
    assert_batch_bits_eq(&direct_epochs, &handle_epochs);

    let broadcast = load_broadcast_fixture("nav/ESBC00DNK_R_20201770000_01D_MN.rnx");
    let gps_record = broadcast
        .records()
        .iter()
        .find(|record| record.satellite_id.system == GnssSystem::Gps)
        .expect("GPS broadcast record");
    let broadcast_sat = gps_record.satellite_id;
    let broadcast_epoch = toe_j2000_s(gps_record);
    let missing_broadcast = sbas(20);
    assert!(
        broadcast
            .observable_state_at_j2000_s(missing_broadcast, broadcast_epoch)
            .is_err(),
        "fixture unexpectedly serves the missing broadcast satellite"
    );

    let broadcast_sats = [broadcast_sat, missing_broadcast, broadcast_sat];
    let broadcast_shared =
        observable_states_at_shared_j2000_s(&broadcast, &broadcast_sats, broadcast_epoch);
    let broadcast_shared_epochs = vec![broadcast_epoch; broadcast_sats.len()];
    assert_batch_matches_scalar(
        &broadcast,
        &broadcast_sats,
        &broadcast_shared_epochs,
        &broadcast_shared,
    );
    assert_eq!(
        broadcast_shared.element_status(1),
        Some(ObservableStateElementStatus::Gap)
    );

    let broadcast_epochs = [broadcast_epoch, broadcast_epoch, broadcast_epoch + 60.0];
    let broadcast_parallel =
        observable_states_at_j2000_s(&broadcast, &broadcast_sats, &broadcast_epochs)
            .expect("matching broadcast batch lengths");
    assert_batch_matches_scalar(
        &broadcast,
        &broadcast_sats,
        &broadcast_epochs,
        &broadcast_parallel,
    );
}
