use sidereon_core::astro::time::model::{GnssWeekTow, JulianDateSplit, TimeScale};
use sidereon_core::astro::time::split_julian_date;
use sidereon_core::combinations::{ionosphere_free, ionosphere_free_phase_cycles};
use sidereon_core::constants::{F_L1_HZ, F_L2_HZ, GPS_EPOCH_TO_J2000_S, SECONDS_PER_WEEK};
use sidereon_core::ephemeris::{BroadcastEphemeris, EphemerisSource, Sp3};
use sidereon_core::observables::{j2000_seconds_from_split, predict, PredictOptions};
use sidereon_core::ppp_corrections::CivilDateTime;
use sidereon_core::precise_positioning::{
    solve_float_epochs, FloatEpoch, FloatObservation, FloatSolveConfig, FloatSolveOptions,
    FloatState, MeasurementWeights, RangeCorrections, TroposphereOptions,
};
use sidereon_core::rinex::observations::{
    observation_values, ObsEpoch, ObsEpochTime, ObservationFilter, RinexObs,
};
use sidereon_core::rtcm::{
    Message, SsrClockRecord, SsrHeader, SsrKind, SsrMessage, SsrOrbitRecord, SsrStreamAssembler,
};
use sidereon_core::ssr::SsrCorrectionStore;
use sidereon_core::{GnssSatelliteId, GnssSystem};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

fn fixture_path(parts: &[&str]) -> PathBuf {
    parts.iter().fold(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
        |path, part| path.join(part),
    )
}

fn load_text(parts: &[&str]) -> String {
    let path = fixture_path(parts);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read fixture {path:?}: {err}"))
}

fn load_sp3() -> Sp3 {
    let path = fixture_path(&["sp3", "GBM0MGXRAP_20201770000_01D_05M_ORB_120epoch.sp3"]);
    let bytes = std::fs::read(&path).unwrap_or_else(|err| panic!("read fixture {path:?}: {err}"));
    Sp3::parse(&bytes).unwrap_or_else(|err| panic!("parse SP3 {path:?}: {err}"))
}

fn load_obs() -> RinexObs {
    RinexObs::parse(&load_text(&[
        "obs",
        "ESBC00DNK_R_20201770000_01D_30S_MO_120epoch.rnx",
    ]))
    .expect("parse ESBC observation fixture")
}

fn load_broadcast() -> BroadcastEphemeris {
    BroadcastEphemeris::from_nav(&load_text(&["nav", "ESBC00DNK_R_20201770000_01D_MN.rnx"]))
        .expect("parse ESBC broadcast NAV fixture")
}

fn gps_l1_l2_filter() -> ObservationFilter {
    ObservationFilter::from_entries([(
        GnssSystem::Gps,
        vec![
            "C1C".to_string(),
            "C2W".to_string(),
            "L1C".to_string(),
            "L2W".to_string(),
        ],
    )])
}

fn civil_to_julian_split(epoch: ObsEpochTime) -> JulianDateSplit {
    let (jd_whole, fraction) = split_julian_date(
        epoch.year,
        i32::from(epoch.month),
        i32::from(epoch.day),
        i32::from(epoch.hour),
        i32::from(epoch.minute),
        epoch.second,
    );
    JulianDateSplit::new(jd_whole, fraction).expect("valid split Julian date")
}

fn civil_datetime(epoch: ObsEpochTime) -> CivilDateTime {
    CivilDateTime {
        year: epoch.year,
        month: epoch.month,
        day: epoch.day,
        hour: epoch.hour,
        minute: epoch.minute,
        second: epoch.second,
    }
}

fn float_observations(epoch: &ObsEpoch, obs: &RinexObs) -> Vec<FloatObservation> {
    let mut out = observation_values(obs, epoch, &gps_l1_l2_filter())
        .expect("valid observation values")
        .into_iter()
        .filter_map(|(sat, rows)| {
            let token = sat.to_string();
            let mut values = BTreeMap::new();
            for row in rows {
                values.insert(row.code, row.value);
            }
            let code_m = ionosphere_free(
                values.get("C1C").and_then(|v| *v)?,
                values.get("C2W").and_then(|v| *v)?,
                F_L1_HZ,
                F_L2_HZ,
            )
            .expect("ionosphere-free code");
            let phase_m = ionosphere_free_phase_cycles(
                values.get("L1C").and_then(|v| *v)?,
                values.get("L2W").and_then(|v| *v)?,
                F_L1_HZ,
                F_L2_HZ,
            )
            .expect("ionosphere-free carrier phase");
            Some(FloatObservation {
                sat,
                satellite_id: token.clone(),
                ambiguity_id: token,
                code_m,
                phase_m,
                freq1_hz: F_L1_HZ,
                freq2_hz: F_L2_HZ,
                glonass_channel: None,
            })
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| a.satellite_id.cmp(&b.satellite_id));
    out
}

fn float_epoch(epoch: ObsEpochTime, observations: Vec<FloatObservation>) -> FloatEpoch {
    let split = civil_to_julian_split(epoch);
    FloatEpoch {
        epoch: civil_datetime(epoch),
        jd_whole: split.jd_whole,
        jd_fraction: split.fraction,
        t_rx_j2000_s: j2000_seconds_from_split(split.jd_whole, split.fraction)
            .expect("valid split Julian date"),
        observations,
    }
}

fn first_gps_epoch(obs: &RinexObs) -> FloatEpoch {
    let epoch = obs.epochs().first().expect("fixture has an epoch");
    let observations = float_observations(epoch, obs)
        .into_iter()
        .filter(|obs| matches!(obs.sat.system, GnssSystem::Gps))
        .collect::<Vec<_>>();
    assert!(
        observations.len() >= 6,
        "fixture epoch has only {} GPS L1/L2 rows",
        observations.len()
    );
    float_epoch(epoch.epoch, observations)
}

fn initial_state(epochs: &[FloatEpoch], approx: [f64; 3]) -> FloatState {
    FloatState {
        position_m: [approx[0] + 100.0, approx[1] - 100.0, approx[2] + 100.0],
        clocks_m: vec![0.0; epochs.len()],
        ambiguities_m: initial_ambiguities(epochs),
        ztd_m: 0.0,
    }
}

fn initial_ambiguities(epochs: &[FloatEpoch]) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    for obs in epochs.iter().flat_map(|epoch| &epoch.observations) {
        out.entry(obs.ambiguity_id.clone())
            .or_insert(obs.phase_m - obs.code_m);
    }
    out
}

fn float_config() -> FloatSolveConfig {
    FloatSolveConfig {
        weights: MeasurementWeights {
            code: 1.0,
            phase: 100.0,
            elevation_weighting: false,
        },
        tropo: TroposphereOptions::disabled(),
        corrections: RangeCorrections::disabled(),
        opts: FloatSolveOptions {
            max_iterations: 8,
            position_tolerance_m: 1.0e-4,
            clock_tolerance_m: 1.0e-4,
            ambiguity_tolerance_m: 1.0e-4,
            ztd_tolerance_m: 1.0e-4,
        },
        residual_screen: false,
    }
}

fn position_error_m(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn synthetic_ssr_store(
    broadcast: &BroadcastEphemeris,
    sp3: &Sp3,
    epoch: &FloatEpoch,
    receiver_m: [f64; 3],
) -> SsrCorrectionStore {
    let mut orbit = Vec::new();
    let mut clock = Vec::new();
    let mut used = BTreeSet::new();
    for obs in &epoch.observations {
        if !used.insert(obs.sat) {
            continue;
        }
        let prediction = predict(
            sp3,
            obs.sat,
            receiver_m,
            epoch.t_rx_j2000_s,
            PredictOptions {
                carrier_hz: F_L1_HZ,
                light_time: true,
                sagnac: true,
            },
        )
        .expect("SP3 prediction");
        let t_tx = prediction.transmit_time_j2000_s;
        let record = broadcast
            .select_record_at(obs.sat, t_tx)
            .expect("broadcast record at transmit time");
        let (broadcast_position, broadcast_clock) = broadcast
            .position_clock_at_j2000_s(obs.sat, t_tx)
            .expect("broadcast state at transmit time");
        let sp3_state = sp3
            .position_at_j2000_seconds(obs.sat, t_tx)
            .expect("SP3 state at transmit time");
        let sp3_position = sp3_state.position.as_array();
        let sp3_clock = sp3_state.clock_s.expect("SP3 clock");
        let velocity = finite_difference_broadcast_velocity(broadcast, obs.sat, t_tx);
        let (er, ea, ec) = velocity_aligned_basis(broadcast_position, velocity);
        let delta = [
            sp3_position[0] - broadcast_position[0],
            sp3_position[1] - broadcast_position[1],
            sp3_position[2] - broadcast_position[2],
        ];
        let radial = dot(delta, er);
        let along = dot(delta, ea);
        let cross = dot(delta, ec);
        orbit.push(SsrOrbitRecord {
            satellite_id: obs.sat.prn,
            iode: record.issue_of_data.issue,
            delta_radial: raw_rtcm_orbit(-radial, 1.0e-4),
            delta_along: raw_rtcm_orbit(-along, 4.0e-4),
            delta_cross: raw_rtcm_orbit(-cross, 4.0e-4),
            dot_delta_radial: 0,
            dot_delta_along: 0,
            dot_delta_cross: 0,
        });
        clock.push(SsrClockRecord {
            satellite_id: obs.sat.prn,
            c0: raw_rtcm_orbit(
                (broadcast_clock - sp3_clock) * sidereon_core::constants::C_M_S,
                1.0e-4,
            ),
            c1: 0,
            c2: 0,
        });
    }

    let tow = epoch.t_rx_j2000_s + GPS_EPOCH_TO_J2000_S;
    let week = (tow / SECONDS_PER_WEEK).floor() as u32;
    let tow_s = tow - f64::from(week) * SECONDS_PER_WEEK;
    let message = Message::Ssr(SsrMessage {
        message_number: 1060,
        system: GnssSystem::Gps,
        kind: SsrKind::CombinedOrbitClock,
        header: SsrHeader {
            epoch_time_s: tow_s.round() as u32,
            update_interval: 0,
            multiple_message: false,
            iod_ssr: 1,
            provider_id: 42,
            solution_id: 1,
            satellite_reference_datum: Some(false),
            dispersive_bias_consistency: None,
            mw_consistency: None,
            satellite_count: orbit.len() as u8,
        },
        orbit,
        clock,
        code_bias: Vec::new(),
        phase_bias: Vec::new(),
        ura: Vec::new(),
        padding_bits: Vec::new(),
    });
    let frame = message.to_frame().expect("valid synthetic SSR frame");
    let mut assembler = SsrStreamAssembler::new();
    let mut store = SsrCorrectionStore::new();
    let week_tow = GnssWeekTow::new(TimeScale::Gpst, week, tow_s).expect("valid SSR week/TOW");
    for decoded in assembler.push(&frame) {
        let decoded = decoded.expect("decode synthetic SSR frame");
        store.ingest(&decoded, week_tow).expect("ingest SSR");
    }
    assert_eq!(assembler.retained_len(), 0);
    store
}

fn raw_rtcm_orbit(value_m: f64, scale: f64) -> i32 {
    (value_m / scale).round() as i32
}

fn finite_difference_broadcast_velocity(
    broadcast: &BroadcastEphemeris,
    sat: GnssSatelliteId,
    t_j2000_s: f64,
) -> [f64; 3] {
    let p_plus = broadcast
        .position_clock_at_j2000_s(sat, t_j2000_s + 0.5)
        .expect("broadcast plus state")
        .0;
    let p_minus = broadcast
        .position_clock_at_j2000_s(sat, t_j2000_s - 0.5)
        .expect("broadcast minus state")
        .0;
    [
        (p_plus[0] - p_minus[0]) / 1.0,
        (p_plus[1] - p_minus[1]) / 1.0,
        (p_plus[2] - p_minus[2]) / 1.0,
    ]
}

fn velocity_aligned_basis(
    position: [f64; 3],
    velocity: [f64; 3],
) -> ([f64; 3], [f64; 3], [f64; 3]) {
    let along = unit(velocity);
    let cross_track = unit(cross(position, velocity));
    let radial = cross(along, cross_track);
    (radial, along, cross_track)
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let n = dot(v, v).sqrt();
    [v[0] / n, v[1] / n, v[2] / n]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[test]
fn synthetic_ssr_corrected_broadcast_ppp_moves_toward_sp3_solution() {
    let sp3 = load_sp3();
    let broadcast = load_broadcast();
    let obs = load_obs();
    let approx = obs
        .header()
        .approx_position_m
        .expect("ESBC approx position");
    let epochs = vec![first_gps_epoch(&obs)];
    let reference = solve_float_epochs(
        &sp3,
        &epochs,
        initial_state(&epochs, approx),
        float_config(),
    )
    .expect("SP3 PPP reference solve");
    let broadcast_solution = solve_float_epochs(
        &broadcast,
        &epochs,
        initial_state(&epochs, approx),
        float_config(),
    )
    .expect("broadcast PPP solve");
    let store = synthetic_ssr_store(&broadcast, &sp3, &epochs[0], reference.position_m);
    let corrected = sidereon_core::ssr::SsrCorrectedEphemeris::new(&broadcast, &store);
    let ssr_solution = solve_float_epochs(
        &corrected,
        &epochs,
        initial_state(&epochs, approx),
        float_config(),
    )
    .expect("SSR-corrected PPP solve");

    let broadcast_error = position_error_m(broadcast_solution.position_m, reference.position_m);
    let ssr_error = position_error_m(ssr_solution.position_m, reference.position_m);
    eprintln!("broadcast_error_m={broadcast_error:.6} ssr_error_m={ssr_error:.6}");
    assert!(
        ssr_error + 0.05 < broadcast_error,
        "SSR error {ssr_error} m must beat broadcast error {broadcast_error} m"
    );
}
