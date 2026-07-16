//! Scoreboard validation tests.
//!
//! Provenance: fixture layout follows the public SP3-c examples already carried
//! in this repository. The synthetic arc is generated from the public two-body
//! propagator and public frame transform APIs to validate the complete harness
//! path without network access.

use serde_json::Value;
use sidereon_core::astro::frames::EarthOrientation;
use sidereon_core::astro::math::least_squares::SolveOptions;
use sidereon_core::astro::propagator::{
    ForceModelKind, IntegratorKind, IntegratorOptions, StatePropagator,
};
use sidereon_core::astro::state::CartesianState;
use sidereon_core::astro::time::civil::{
    civil_from_j2000_seconds, j2000_seconds, split_julian_date_from_j2000_seconds,
};
use sidereon_core::astro::time::model::{Instant, JulianDateSplit, TimeScale};
use sidereon_core::constants::{J2000_JD, SECONDS_PER_DAY};
use sidereon_core::data::ProductDate;
use sidereon_core::ephemeris::Sp3;
use sidereon_core::{
    EarthOrientationProvider, GnssSatelliteId, GnssSystem, TdbEarthOrientationProvider,
};
use sidereon_scoreboard::{
    parse_product_date, resolve_latest_available_rapid_sp3, run_with_fetcher, score_sp3_bytes,
    FetchOutcome, HttpsFetcher, ProductCandidate, ProductFetcher, ScoreOptions, ScoreboardStatus,
};

const SP3_POSITION_3D_QUANTIZATION_BOUND_M: f64 = 8.660_254_037_844_386e-4;

fn date(year: i32, month: u8, day: u8) -> ProductDate {
    ProductDate::new(year, month, day).expect("valid date")
}

#[test]
fn fixture_schema_is_exact_and_counts_add_up() {
    let bytes = include_bytes!("fixtures/minimal_sp3.sp3");
    let report = score_sp3_bytes(
        bytes,
        "fixture.sp3",
        date(2020, 6, 24),
        &ScoreOptions::default(),
    )
    .expect("fixture scores");
    let value = serde_json::to_value(&report).expect("report JSON");
    assert_keys(
        &value,
        &[
            "attempted_candidates",
            "date_utc",
            "notes",
            "per_constellation",
            "per_sat",
            "product",
            "sidereon_version",
            "status",
        ],
    );
    assert_eq!(value["status"], "scored");
    assert_keys(
        &value["product"],
        &["agency", "name", "parser_skipped_records"],
    );
    assert_keys(&value["per_sat"], &["bottom", "skipped", "top"]);
    let gps = &value["per_constellation"]["GPS"];
    assert_keys(
        gps,
        &[
            "fit_count",
            "median_rms_3d_m",
            "sat_count",
            "skipped",
            "worst_rms_3d_m",
        ],
    );
    let sat_count = gps["sat_count"].as_u64().unwrap();
    let fit_count = gps["fit_count"].as_u64().unwrap();
    let skipped = gps["skipped"].as_u64().unwrap();
    assert_eq!(fit_count + skipped, sat_count);
    assert_eq!(report.per_sat.skipped.len(), 1);
    assert_eq!(
        report
            .product
            .as_ref()
            .expect("scored report has product")
            .parser_skipped_records,
        0
    );
    let skipped = serde_json::to_value(&report.per_sat.skipped[0]).expect("skip row JSON");
    assert_keys(&skipped, &["constellation", "reason", "satellite"]);
}

#[test]
fn parser_skipped_records_are_visible() {
    let bytes = include_str!("fixtures/minimal_sp3.sp3")
        .replace("+    1   G01  0", "+    2   G01R28  0")
        .into_bytes();
    let report = score_sp3_bytes(
        &bytes,
        "fixture-with-unsupported.sp3",
        date(2020, 6, 24),
        &ScoreOptions::default(),
    )
    .expect("fixture scores");

    assert_eq!(
        report
            .product
            .as_ref()
            .expect("scored report has product")
            .parser_skipped_records,
        1
    );
    assert!(report
        .notes
        .iter()
        .any(|note| note.contains("product.parser_skipped_records")));
}

#[test]
fn mocked_fetch_resolves_without_network() {
    struct MockFetcher;

    impl ProductFetcher for MockFetcher {
        fn fetch(
            &self,
            candidate: &ProductCandidate,
        ) -> Result<FetchOutcome, sidereon_scoreboard::ScoreboardError> {
            if candidate.name.contains("20201760000") {
                Ok(FetchOutcome::Available(
                    include_bytes!("fixtures/minimal_sp3.sp3").to_vec(),
                ))
            } else {
                Ok(FetchOutcome::NotPosted { http_status: None })
            }
        }
    }

    let resolution = resolve_latest_available_rapid_sp3(date(2020, 6, 25), 1, &MockFetcher)
        .expect("previous day resolves");
    let resolved = resolution.resolved.expect("resolved product");
    assert!(resolved.candidate.name.contains("20201760000"));
    assert_eq!(
        resolved.bytes,
        include_bytes!("fixtures/minimal_sp3.sp3").to_vec()
    );
    assert!(resolution
        .attempted
        .iter()
        .any(|candidate| candidate.name == "IGS0OPSRAP_20201760000_01D_15M_ORB.SP3"));
}

#[test]
fn missing_candidate_urls_are_no_data_report() {
    struct MissingFetcher;

    impl ProductFetcher for MissingFetcher {
        fn fetch(
            &self,
            _candidate: &ProductCandidate,
        ) -> Result<FetchOutcome, sidereon_scoreboard::ScoreboardError> {
            Ok(FetchOutcome::NotPosted {
                http_status: Some(404),
            })
        }
    }

    let report = run_with_fetcher(date(2026, 7, 5), 0, &MissingFetcher).expect("no-data report");
    assert_eq!(report.status, ScoreboardStatus::NoData);
    assert!(report.product.is_none());
    assert_eq!(report.attempted_candidates.len(), 19);
    assert!(report.per_constellation.is_empty());
    assert!(report.per_sat.top.is_empty());
    assert!(report
        .attempted_candidates
        .iter()
        .any(|candidate| candidate.url.contains("/products/2426/")));
    assert!(report
        .attempted_candidates
        .iter()
        .all(|candidate| candidate.http_status == Some(404)));
    assert!(report
        .notes
        .iter()
        .any(|note| note.contains("attempted URL")));
    let attempted =
        serde_json::to_value(&report.attempted_candidates[0]).expect("attempted candidate JSON");
    assert_keys(
        &attempted,
        &[
            "cadence",
            "date_utc",
            "http_status",
            "name",
            "source",
            "url",
        ],
    );
}

#[test]
fn candidate_urls_use_product_dates_gps_week() {
    struct CaptureFetcher;

    impl ProductFetcher for CaptureFetcher {
        fn fetch(
            &self,
            candidate: &ProductCandidate,
        ) -> Result<FetchOutcome, sidereon_scoreboard::ScoreboardError> {
            if candidate.name == "IGS0OPSRAP_20261850000_01D_15M_ORB.SP3" {
                Ok(FetchOutcome::Available(
                    include_bytes!("fixtures/minimal_sp3.sp3").to_vec(),
                ))
            } else {
                Ok(FetchOutcome::NotPosted { http_status: None })
            }
        }
    }

    let resolution = resolve_latest_available_rapid_sp3(date(2026, 7, 5), 1, &CaptureFetcher)
        .expect("previous GPS week resolves");
    let resolved = resolution.resolved.expect("resolved product");
    assert_eq!(
        resolved.candidate.url,
        "https://igs.bkg.bund.de/root_ftp/IGS/products/2425/IGS0OPSRAP_20261850000_01D_15M_ORB.SP3.gz"
    );
}

#[test]
#[ignore = "network test for current public SP3 archives"]
fn live_current_product_candidate_resolves() {
    let target = sidereon_scoreboard::utc_today().expect("UTC date");
    let resolution = resolve_latest_available_rapid_sp3(target, 4, &HttpsFetcher)
        .expect("live resolver does not fail");
    assert!(
        resolution.resolved.is_some(),
        "no posted product in {} attempts: {:#?}",
        resolution.attempted.len(),
        resolution
            .attempted
            .iter()
            .map(|candidate| &candidate.url)
            .collect::<Vec<_>>()
    );
}

#[test]
fn synthetic_state_arc_runs_full_path_to_near_zero_rms() {
    let start = j2000_seconds(2026, 6, 1, 0, 0, 0.0) as i64;
    let epochs: Vec<i64> = (0..=8).map(|step| start + step * 60).collect();
    let initial = CartesianState::new(start as f64, [7078.0, -30.0, 820.0], [0.20, 7.35, 1.05]);
    let sp3 = synthetic_sp3(initial, &epochs);
    let mut options = ScoreOptions::default();
    options.fit_options.force_model = ForceModelKind::two_body();
    options.fit_options.integrator = IntegratorKind::Dp54;
    options.fit_options.integrator_options = IntegratorOptions {
        abs_tol: 1.0e-12,
        rel_tol: 1.0e-13,
        initial_step: 10.0,
        max_step: 60.0,
        ..IntegratorOptions::default()
    };
    options.fit_options.solver_options = SolveOptions {
        gtol: 1.0e-15,
        ftol: 1.0e-15,
        xtol: 1.0e-15,
        max_nfev: 1200,
    };

    let report = score_sp3_bytes(sp3.as_bytes(), "synthetic.sp3", date(2026, 6, 1), &options)
        .expect("synthetic arc scores");
    let parsed = Sp3::parse(sp3.as_bytes()).expect("synthetic SP3 parses");
    assert_eq!(parsed.precise_ephemeris_state_samples().len(), epochs.len());
    assert!(!report
        .notes
        .iter()
        .any(|note| note.contains("position-sample fitter")));
    let top = report.per_sat.top.first().expect("fit row");
    let top_json = serde_json::to_value(top).expect("top row JSON");
    assert_keys(&top_json, FIT_ROW_KEYS);
    let bottom_json = serde_json::to_value(report.per_sat.bottom.first().expect("bottom row"))
        .expect("bottom row JSON");
    assert_keys(&bottom_json, FIT_ROW_KEYS);
    assert!(
        top.rms_3d_m < SP3_POSITION_3D_QUANTIZATION_BOUND_M,
        "synthetic RMS was {:.17e} m",
        top.rms_3d_m
    );
    assert!(report.per_sat.skipped.is_empty());
}

#[test]
fn partial_velocity_arc_is_reported_as_skip() {
    let start = j2000_seconds(2026, 6, 1, 0, 0, 0.0) as i64;
    let epochs: Vec<i64> = (0..=8).map(|step| start + step * 60).collect();
    let initial = CartesianState::new(start as f64, [7078.0, -30.0, 820.0], [0.20, 7.35, 1.05]);
    let sp3 = blank_first_velocity_record(&synthetic_sp3(initial, &epochs));
    let mut options = ScoreOptions::default();
    options.fit_options.force_model = ForceModelKind::two_body();

    let report = score_sp3_bytes(
        sp3.as_bytes(),
        "partial-velocity.sp3",
        date(2026, 6, 1),
        &options,
    )
    .expect("partial velocity arc scores");

    assert!(report.per_sat.top.is_empty());
    assert_eq!(report.per_sat.skipped.len(), 1);
    assert_eq!(
        report.per_sat.skipped[0].reason,
        "partial_velocity_samples:8/9"
    );
    let gps = report.per_constellation.get("GPS").expect("GPS report");
    assert_eq!(gps.sat_count, 1);
    assert_eq!(gps.fit_count, 0);
    assert_eq!(gps.skipped, 1);
}

#[test]
fn date_parser_rejects_extra_fields() {
    assert!(parse_product_date("2026-07-04-extra").is_err());
}

fn assert_keys(value: &Value, expected: &[&str]) {
    let object = value.as_object().expect("JSON object");
    let mut keys = object.keys().map(String::as_str).collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(keys, expected);
}

const FIT_ROW_KEYS: &[&str] = &[
    "along_rms_m",
    "constellation",
    "cross_rms_m",
    "low_sample_count",
    "n",
    "radial_rms_m",
    "rms_3d_m",
    "satellite",
];

fn synthetic_sp3(initial: CartesianState, epochs_j2000_s: &[i64]) -> String {
    let sat = GnssSatelliteId::new(GnssSystem::Gps, 1).expect("valid satellite");
    let propagator = StatePropagator {
        initial,
        force_model: ForceModelKind::two_body(),
        integrator: IntegratorKind::Dp54,
        options: IntegratorOptions {
            abs_tol: 1.0e-12,
            rel_tol: 1.0e-13,
            initial_step: 10.0,
            max_step: 60.0,
            ..IntegratorOptions::default()
        },
        drag: None,
        space_weather: None,
    };
    let query_epochs = epochs_j2000_s
        .iter()
        .map(|&epoch| epoch as f64)
        .collect::<Vec<_>>();
    let states = propagator.ephemeris(&query_epochs).expect("truth arc");
    let provider = TdbEarthOrientationProvider::new();
    let mut out = String::new();
    out.push_str(&format!(
        "#cV{} {:>7} ORBIT IGS14 FIT  TST\n",
        format_calendar(2026, 6, 1, 0, 0, 0.0),
        epochs_j2000_s.len()
    ));
    out.push_str("## 2421  86400.00000000    60.00000000 61192 0.0000000000000\n");
    out.push_str("+    1   G01  0  0  0  0  0  0  0  0  0  0  0  0  0  0  0  0\n");
    out.push_str("++         0  0  0  0  0  0  0  0  0  0  0  0  0  0  0  0  0\n");
    out.push_str("%c M  cc GPS ccc cccc cccc cccc cccc ccccc ccccc ccccc ccccc\n");
    out.push_str("%c cc cc ccc ccc cccc cccc cccc cccc ccccc ccccc ccccc ccccc\n");
    out.push_str("%f  1.2500000  1.025000000  0.00000000000  0.000000000000000\n");
    out.push_str("%f  0.0000000  0.000000000  0.00000000000  0.000000000000000\n");
    out.push_str("%i    0    0    0    0      0      0      0      0         0\n");
    out.push_str("%i    0    0    0    0      0      0      0      0         0\n");
    for (state, &epoch) in states.iter().zip(epochs_j2000_s) {
        let (year, month, day, hour, minute, second) = civil_from_j2000_seconds(epoch);
        out.push_str(&format!(
            "*  {}\n",
            format_calendar(year, month, day, hour, minute, second as f64)
        ));
        let instant = instant_at(TimeScale::Gpst, epoch);
        let seed = EarthOrientation::from_instant(instant).expect("seed orientation");
        let tdb_seconds = (seed.time_scales().jd_tdb - J2000_JD) * SECONDS_PER_DAY;
        let orientation = provider
            .orientation_at_tdb_seconds(tdb_seconds)
            .expect("orientation");
        let (position_itrf_km, velocity_itrf_km_s) = orientation
            .gcrf_to_itrf_state_km(state.position_array(), state.velocity_array())
            .expect("state transform");
        out.push_str(&format!(
            "P{sat}{:14.6}{:14.6}{:14.6}{:14.6}\n",
            position_itrf_km[0], position_itrf_km[1], position_itrf_km[2], 0.0
        ));
        out.push_str(&format!(
            "V{sat}{:14.6}{:14.6}{:14.6}{:14.6}\n",
            velocity_itrf_km_s[0] * 10_000.0,
            velocity_itrf_km_s[1] * 10_000.0,
            velocity_itrf_km_s[2] * 10_000.0,
            0.0
        ));
    }
    out.push_str("EOF\n");
    out
}

fn blank_first_velocity_record(sp3: &str) -> String {
    let mut replaced = false;
    let lines = sp3
        .lines()
        .map(|line| {
            if !replaced && line.starts_with("VG01") {
                replaced = true;
                format!("VG01{:14.6}{:14.6}{:14.6}{:14.6}", 0.0, 0.0, 0.0, 0.0)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>();
    format!("{}\n", lines.join("\n"))
}

fn instant_at(scale: TimeScale, epoch_j2000_s: i64) -> Instant {
    let (jd_whole, fraction) = split_julian_date_from_j2000_seconds(epoch_j2000_s);
    Instant::from_julian_date(
        scale,
        JulianDateSplit::new(jd_whole, fraction).expect("valid split Julian date"),
    )
}

fn format_calendar(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    seconds: f64,
) -> String {
    format!("{year:4} {month:>2} {day:>2} {hour:>2} {minute:>2} {seconds:11.8}")
}
