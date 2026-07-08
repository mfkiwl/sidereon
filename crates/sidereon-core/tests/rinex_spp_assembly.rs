use std::path::PathBuf;

use sidereon_core::ephemeris::{BroadcastEphemeris, PreciseEphemerisInterpolant, Sp3};
use sidereon_core::positioning::{
    solve_spp_from_rinex_obs, spp_inputs_from_rinex_obs, Corrections, Dop, ReceiverSolution,
    RinexSppOptions, SolvePolicy,
};
use sidereon_core::rinex::observations::{ObservationFile, SignalPolicy};
use sidereon_core::{GnssSatelliteId, GnssSystem};

const RTKLIB_USED_SATS: &[&str] = &[
    "G05", "G07", "G09", "G13", "G15", "G18", "G27", "G28", "G30", "R01", "R02", "R08", "R09",
    "R10", "R11", "R17", "R18",
];
const RTKLIB_REFERENCE_ECEF_M: [f64; 3] = [3582110.6334, 532590.1127, 5232764.8971];

fn fixture_path(parts: &[&str]) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for part in parts {
        path.push(part);
    }
    path
}

fn load_text(parts: &[&str]) -> String {
    let path = fixture_path(parts);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read fixture {path:?}: {err}"))
}

fn load_obs() -> ObservationFile {
    ObservationFile::parse(&load_text(&[
        "obs",
        "ESBC00DNK_R_20201770000_01D_30S_MO_trim.rnx",
    ]))
    .expect("parse ESBC observation fixture")
}

fn load_sp3() -> Sp3 {
    let bytes = std::fs::read(fixture_path(&[
        "sp3",
        "COD0MGXFIN_20201770000_01D_05M_ORB.SP3",
    ]))
    .expect("read COD precise SP3");
    Sp3::parse(&bytes).expect("parse COD precise SP3")
}

fn combined_broadcast_store() -> BroadcastEphemeris {
    let gps = load_text(&["nav", "ESBC00DNK_R_20201770000_01D_MN.rnx"]);
    let glo = load_text(&["nav", "ESBC00DNK_R_20201770000_01D_RN.rnx"]);
    let glo_body = glo
        .split_once("END OF HEADER")
        .map(|(_, body)| body.trim_start_matches(['\r', '\n']))
        .expect("GLONASS nav END OF HEADER");
    BroadcastEphemeris::from_nav(&format!("{gps}{glo_body}"))
        .expect("parse combined GPS and GLONASS NAV")
}

fn satellite_id(token: &str) -> GnssSatelliteId {
    let mut chars = token.chars();
    let system = GnssSystem::from_letter(chars.next().expect("system char"))
        .expect("known GNSS system code");
    let prn = chars.as_str().parse::<u8>().expect("PRN integer");
    GnssSatelliteId::new(system, prn).expect("valid satellite id")
}

fn distance_m(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn assert_f64_bits_eq(left: f64, right: f64) {
    assert_eq!(left.to_bits(), right.to_bits());
}

fn assert_f64_vec_bits_eq(left: &[f64], right: &[f64]) {
    assert_eq!(
        left.iter().map(|value| value.to_bits()).collect::<Vec<_>>(),
        right
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>()
    );
}

fn assert_matrix3_bits_eq(left: &[[f64; 3]; 3], right: &[[f64; 3]; 3]) {
    assert_eq!(
        left.iter()
            .flatten()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>(),
        right
            .iter()
            .flatten()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>()
    );
}

fn assert_geodetic_bits_eq(
    left: Option<sidereon_core::Wgs84Geodetic>,
    right: Option<sidereon_core::Wgs84Geodetic>,
) {
    match (left, right) {
        (Some(left), Some(right)) => {
            assert_f64_bits_eq(left.lat_rad, right.lat_rad);
            assert_f64_bits_eq(left.lon_rad, right.lon_rad);
            assert_f64_bits_eq(left.height_m, right.height_m);
        }
        (None, None) => {}
        _ => panic!("geodetic presence mismatch"),
    }
}

fn assert_dop_bits_eq(left: &Option<Dop>, right: &Option<Dop>) {
    match (left, right) {
        (Some(left), Some(right)) => {
            assert_f64_bits_eq(left.gdop, right.gdop);
            assert_f64_bits_eq(left.pdop, right.pdop);
            assert_f64_bits_eq(left.hdop, right.hdop);
            assert_f64_bits_eq(left.vdop, right.vdop);
            assert_f64_bits_eq(left.tdop, right.tdop);
            assert_eq!(left.system_tdops.len(), right.system_tdops.len());
            for ((left_system, left_tdop), (right_system, right_tdop)) in
                left.system_tdops.iter().zip(right.system_tdops.iter())
            {
                assert_eq!(left_system, right_system);
                assert_f64_bits_eq(*left_tdop, *right_tdop);
            }
        }
        (None, None) => {}
        _ => panic!("DOP presence mismatch"),
    }
}

fn assert_receiver_solution_bits_eq(left: &ReceiverSolution, right: &ReceiverSolution) {
    assert_f64_bits_eq(left.position.x_m, right.position.x_m);
    assert_f64_bits_eq(left.position.y_m, right.position.y_m);
    assert_f64_bits_eq(left.position.z_m, right.position.z_m);
    assert_geodetic_bits_eq(left.geodetic, right.geodetic);
    assert_f64_bits_eq(left.rx_clock_s, right.rx_clock_s);
    assert_eq!(left.rx_clock_drift_s_s, right.rx_clock_drift_s_s);
    assert_eq!(left.system_clocks_s.len(), right.system_clocks_s.len());
    for ((left_system, left_clock), (right_system, right_clock)) in left
        .system_clocks_s
        .iter()
        .zip(right.system_clocks_s.iter())
    {
        assert_eq!(left_system, right_system);
        assert_f64_bits_eq(*left_clock, *right_clock);
    }
    assert_dop_bits_eq(&left.dop, &right.dop);
    assert_eq!(left.system_tdops.len(), right.system_tdops.len());
    for ((left_system, left_tdop), (right_system, right_tdop)) in
        left.system_tdops.iter().zip(right.system_tdops.iter())
    {
        assert_eq!(left_system, right_system);
        assert_f64_bits_eq(*left_tdop, *right_tdop);
    }
    assert_matrix3_bits_eq(
        &left.position_covariance.ecef_m2,
        &right.position_covariance.ecef_m2,
    );
    assert_matrix3_bits_eq(
        &left.position_covariance.enu_m2,
        &right.position_covariance.enu_m2,
    );
    assert_f64_vec_bits_eq(&left.residuals_m, &right.residuals_m);
    assert_eq!(left.used_sats, right.used_sats);
    assert_eq!(left.rejected_sats, right.rejected_sats);
    assert_eq!(left.geometry_quality.tier, right.geometry_quality.tier);
    assert_eq!(
        left.geometry_quality.redundancy,
        right.geometry_quality.redundancy
    );
    assert_eq!(left.geometry_quality.rank, right.geometry_quality.rank);
    assert_f64_bits_eq(
        left.geometry_quality.condition_number,
        right.geometry_quality.condition_number,
    );
    assert_f64_bits_eq(left.geometry_quality.gdop, right.geometry_quality.gdop);
    assert_eq!(
        left.geometry_quality.raim_checkable,
        right.geometry_quality.raim_checkable
    );
    assert_eq!(
        left.geometry_quality.covariance_validated,
        right.geometry_quality.covariance_validated
    );
    assert_eq!(left.metadata, right.metadata);
}

#[test]
fn spp_inputs_from_rinex_obs_assembles_esbc_first_epoch() {
    let obs = load_obs();
    let sp3 = load_sp3();
    let options = RinexSppOptions::new(SignalPolicy {
        codes: [(GnssSystem::Gps, vec!["C1C".to_string()])].into(),
    })
    .with_corrections(Corrections {
        ionosphere: false,
        troposphere: true,
    });

    let epochs = spp_inputs_from_rinex_obs(&obs, &sp3, &options).expect("assemble SPP inputs");
    assert_eq!(epochs.len(), 2);

    let first = &epochs[0];
    assert_eq!(first.epoch_index, 0);
    assert_eq!(
        (first.epoch.year, first.epoch.month, first.epoch.day),
        (2020, 6, 25)
    );
    assert_eq!((first.epoch.hour, first.epoch.minute), (0, 0));
    assert_eq!(first.inputs.observations.len(), 12);
    assert!(first
        .inputs
        .observations
        .iter()
        .all(|obs| obs.satellite_id.system == GnssSystem::Gps));
    assert_eq!(
        first.inputs.initial_guess,
        [3582105.2910, 532589.7313, 5232754.8054, 0.0]
    );
    assert!(first.inputs.corrections.troposphere);
    assert!(!first.inputs.corrections.ionosphere);
    assert_eq!(first.inputs.glonass_channels.len(), 23);
    assert_eq!(first.inputs.glonass_channels.get(&1), Some(&1));
    assert_eq!(first.inputs.glonass_channels.get(&10), Some(&-7));
    assert_eq!(first.inputs.t_rx_j2000_s, 646315200.0);
    assert_eq!(first.inputs.t_rx_second_of_day_s, 0.0);
    assert_eq!(first.inputs.day_of_year, 177.0);
}

#[test]
fn staged_precise_interpolant_spp_is_bit_identical_to_sp3() {
    let obs = load_obs();
    let sp3 = load_sp3();
    let precise = PreciseEphemerisInterpolant::from_sp3(&sp3);
    let options = RinexSppOptions::new(SignalPolicy {
        codes: [(GnssSystem::Gps, vec!["C1C".to_string()])].into(),
    })
    .with_corrections(Corrections {
        ionosphere: false,
        troposphere: true,
    });

    let epochs = spp_inputs_from_rinex_obs(&obs, &sp3, &options).expect("assemble SPP inputs");
    assert!(!epochs.is_empty());
    for epoch in epochs {
        let sp3_solution =
            sidereon_core::positioning::solve(&sp3, &epoch.inputs, true).expect("SP3 solve");
        let staged_solution = sidereon_core::positioning::solve(&precise, &epoch.inputs, true)
            .expect("staged precise solve");
        assert_receiver_solution_bits_eq(&sp3_solution, &staged_solution);
    }
}

#[test]
fn solve_spp_from_rinex_obs_matches_rtklib_glonass_reference() {
    let obs = load_obs();
    let store = combined_broadcast_store();
    let options = RinexSppOptions::default_for(&obs)
        .expect("default signal policy")
        .with_corrections(Corrections::IONO)
        .with_satellites(RTKLIB_USED_SATS.iter().map(|sat| satellite_id(sat)));

    let solutions = solve_spp_from_rinex_obs(&store, &obs, &options, true, SolvePolicy::default())
        .expect("assemble and solve RINEX SPP");
    assert_eq!(solutions.len(), 2);

    let first = solutions[0]
        .solution
        .as_ref()
        .expect("first epoch broadcast SPP");
    assert_eq!(first.used_sats.len(), RTKLIB_USED_SATS.len());
    assert!(first
        .used_sats
        .iter()
        .any(|sat| sat.system == GnssSystem::Glonass));
    assert_eq!(first.system_clocks_s.len(), 2);

    let delta = distance_m(first.position.as_array(), RTKLIB_REFERENCE_ECEF_M);
    assert!(
        delta < 2.0,
        "GLONASS SPP position delta to RTKLIB-demo5 is {delta:.4} m"
    );
}
