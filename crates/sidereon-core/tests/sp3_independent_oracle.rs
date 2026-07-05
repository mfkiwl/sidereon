//! Independent SP3 evaluation oracle.
//!
//! Fixture: `tests/fixtures/sp3/COD0MGXFIN_20201770000_01D_05M_ORB.SP3`,
//! a public IGS MGEX final orbit and clock product for 2020-06-25, committed
//! in this crate's existing SP3 fixture set. The checks below read the SP3 text
//! records directly with fixed-column parsing in the test. Expected positions
//! and clocks are not taken from `Sp3::state`, `Sp3::position_at_j2000_seconds`,
//! or the cached interpolant.

use sidereon_core::astro::time::civil::{j2000_seconds, j2000_seconds_from_split};
use sidereon_core::astro::time::model::{Instant, JulianDateSplit, TimeScale};
use sidereon_core::constants::{J2000_JD, SECONDS_PER_DAY};
use sidereon_core::ephemeris::{
    observable_states_at_j2000_s, PreciseEphemerisInterpolant, PreciseEphemerisSample,
    PreciseEphemerisSamples, Sp3,
};
use sidereon_core::{GnssSatelliteId, GnssSystem};

const COD_5M_FIXTURE: &str = "tests/fixtures/sp3/COD0MGXFIN_20201770000_01D_05M_ORB.SP3";
const GAP_15M_FIXTURE: &str = "tests/fixtures/sp3/GAP_G01_20201760000_15M.sp3";
const SP3_POSITION_RESOLUTION_M: f64 = 1.0e-3;
const SP3_CLOCK_RESOLUTION_S: f64 = 1.0e-12;

#[derive(Debug, Clone, Copy)]
struct TextEpoch {
    j2000_s: f64,
    is_boundary_node: bool,
}

#[derive(Debug, Clone, Copy)]
struct TextRecord {
    epoch: TextEpoch,
    sat: GnssSatelliteId,
    position_m: [f64; 3],
    clock_s: Option<f64>,
}

fn gps(prn: u8) -> GnssSatelliteId {
    GnssSatelliteId::new(GnssSystem::Gps, prn).expect("valid GPS satellite")
}

fn fixture_text(name: &str) -> String {
    std::fs::read_to_string(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name))
        .expect("read SP3 fixture")
}

fn parse_epoch(line: &str) -> TextEpoch {
    let fields: Vec<_> = line[1..].split_whitespace().collect();
    assert_eq!(fields.len(), 6, "unexpected SP3 epoch line: {line}");
    let year = fields[0].parse::<i32>().expect("year");
    let month = fields[1].parse::<i32>().expect("month");
    let day = fields[2].parse::<i32>().expect("day");
    let hour = fields[3].parse::<i32>().expect("hour");
    let minute = fields[4].parse::<i32>().expect("minute");
    let second = fields[5].parse::<f64>().expect("second");
    let j2000_s = j2000_seconds(year, month, day, hour, minute, second);
    let whole = j2000_s.round() as i64;
    TextEpoch {
        j2000_s,
        is_boundary_node: whole.rem_euclid(2700) == 1800,
    }
}

fn parse_record(line: &str, epoch: TextEpoch) -> Option<TextRecord> {
    if !line.starts_with('P') {
        return None;
    }
    let sat = line.get(1..4)?.trim().parse::<GnssSatelliteId>().ok()?;
    let x_km = line.get(4..18)?.trim().parse::<f64>().expect("x km");
    let y_km = line.get(18..32)?.trim().parse::<f64>().expect("y km");
    let z_km = line.get(32..46)?.trim().parse::<f64>().expect("z km");
    if x_km == 0.0 && y_km == 0.0 && z_km == 0.0 {
        return None;
    }
    let clock_us = line
        .get(46..60)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.parse::<f64>().expect("clock us"))
        .filter(|value| value.abs() < 999_999.999_999);
    Some(TextRecord {
        epoch,
        sat,
        position_m: [x_km * 1000.0, y_km * 1000.0, z_km * 1000.0],
        clock_s: clock_us.map(|value| value * 1.0e-6),
    })
}

fn text_records(text: &str, sat: GnssSatelliteId) -> Vec<TextRecord> {
    let mut current_epoch = None;
    let mut out = Vec::new();
    for line in text.lines() {
        if line.starts_with('*') {
            current_epoch = Some(parse_epoch(line));
        } else if let Some(epoch) = current_epoch {
            if let Some(record) = parse_record(line, epoch).filter(|record| record.sat == sat) {
                out.push(record);
            }
        }
    }
    out
}

fn floor_sensitive_45m_fixture_text() -> String {
    let header_src = fixture_text(GAP_15M_FIXTURE);
    let epoch_start = header_src.find("\n*  ").expect("first epoch line") + 1;
    let mut text = String::from(&header_src[..epoch_start]);

    for i in 0..12 {
        let second_of_day = 6 * 3600 + 7 + i * 2700;
        let hour = second_of_day / 3600;
        let minute = (second_of_day % 3600) / 60;
        let second = second_of_day % 60;
        let x_km = 20_000.0 + 10.0 * i as f64;
        let y_km = 14_000.0 + 7.0 * i as f64;
        let z_km = 21_000.0 - 5.0 * i as f64;
        let clock_us = 100.0 + i as f64;
        text.push_str(&format!(
            "*  2000  1  1 {hour:2} {minute:2} {second:2}.00000000\n"
        ));
        text.push_str(&format!(
            "PG01{x_km:14.6}{y_km:14.6}{z_km:14.6}{clock_us:14.6}\n"
        ));
    }
    text.push_str("EOF\n");
    text
}

fn converted_epoch_one_ulp_low(scale: TimeScale, exact_j2000_s: f64) -> Instant {
    assert!(exact_j2000_s.is_sign_positive());
    assert_eq!(
        (exact_j2000_s as i64).rem_euclid(2700),
        1800,
        "fixture epoch must be on the affected 45-minute boundary"
    );
    let converted = f64::from_bits(exact_j2000_s.to_bits() - 1);
    assert_eq!(
        (converted + 2f64.powi(-24)).to_bits(),
        exact_j2000_s.to_bits(),
        "reported bisection should flip the converted epoch onto the node"
    );

    let day = (converted / SECONDS_PER_DAY).floor();
    let fraction = (converted - day * SECONDS_PER_DAY) / SECONDS_PER_DAY;
    let split =
        JulianDateSplit::new(J2000_JD + day, fraction).expect("valid converted split epoch");
    assert_eq!(
        j2000_seconds_from_split(split.jd_whole, split.fraction).to_bits(),
        converted.to_bits(),
        "test helper must exercise the same split-to-J2000 conversion as sample ingestion"
    );
    Instant::from_julian_date(scale, split)
}

fn converted_epoch_samples(records: &[TextRecord]) -> Vec<PreciseEphemerisSample> {
    records
        .iter()
        .map(|record| {
            PreciseEphemerisSample::new(
                record.sat,
                if record.epoch.is_boundary_node {
                    converted_epoch_one_ulp_low(TimeScale::Gpst, record.epoch.j2000_s)
                } else {
                    let day = (record.epoch.j2000_s / SECONDS_PER_DAY).floor();
                    let fraction = (record.epoch.j2000_s - day * SECONDS_PER_DAY) / SECONDS_PER_DAY;
                    Instant::from_julian_date(
                        TimeScale::Gpst,
                        JulianDateSplit::new(J2000_JD + day, fraction)
                            .expect("valid exact split epoch"),
                    )
                },
                record.position_m,
                record.clock_s,
            )
        })
        .collect()
}

fn assert_record_matches(
    record: TextRecord,
    state_position_m: [f64; 3],
    state_clock_s: Option<f64>,
) {
    for (axis, &got) in state_position_m.iter().enumerate() {
        let delta_m = (got - record.position_m[axis]).abs();
        assert!(
            delta_m <= 0.5 * SP3_POSITION_RESOLUTION_M,
            "axis {axis} at {} s differs from SP3 text by {delta_m:e} m",
            record.epoch.j2000_s
        );
    }
    match (state_clock_s, record.clock_s) {
        (Some(got), Some(want)) => {
            let delta_s = (got - want).abs();
            assert!(
                delta_s <= 0.5 * SP3_CLOCK_RESOLUTION_S,
                "clock at {} s differs from SP3 text by {delta_s:e} s",
                record.epoch.j2000_s
            );
        }
        (None, None) => {}
        other => panic!(
            "clock presence mismatch at {} s: {other:?}",
            record.epoch.j2000_s
        ),
    }
}

fn assert_record_matches_all_sample_paths(
    record: TextRecord,
    direct_samples: &PreciseEphemerisSamples,
    cached_samples: &PreciseEphemerisInterpolant,
) {
    let direct = direct_samples
        .position_at_j2000_seconds(record.sat, record.epoch.j2000_s)
        .expect("direct sample-backed state at record epoch");
    assert_record_matches(record, direct.position.as_array(), direct.clock_s);

    let cached = cached_samples
        .position_at_j2000_seconds(record.sat, record.epoch.j2000_s)
        .expect("cached sample-backed state at record epoch");
    assert_record_matches(record, cached.position.as_array(), cached.clock_s);
}

fn assert_state_bits_eq(
    sat: GnssSatelliteId,
    epoch_j2000_s: f64,
    parsed: sidereon_core::ephemeris::Sp3State,
    from_samples: sidereon_core::ephemeris::Sp3State,
) {
    assert_eq!(
        parsed.position.as_array().map(f64::to_bits),
        from_samples.position.as_array().map(f64::to_bits),
        "{sat} position bits differ at {epoch_j2000_s}"
    );
    assert_eq!(
        parsed.clock_s.map(f64::to_bits),
        from_samples.clock_s.map(f64::to_bits),
        "{sat} clock bits differ at {epoch_j2000_s}"
    );
}

#[test]
fn converted_sample_epochs_match_sp3_text_oracle_at_45m_boundaries() {
    let text = fixture_text(COD_5M_FIXTURE);
    let sp3 = Sp3::parse(text.as_bytes()).expect("parse SP3 fixture");
    let sat = gps(1);
    let records: Vec<_> = text_records(&text, sat).into_iter().take(144).collect();
    let boundary_count = records
        .iter()
        .filter(|record| record.epoch.is_boundary_node)
        .count();
    assert_eq!(
        boundary_count, 16,
        "fixture must retain the affected 16/144 boundary-node coverage"
    );

    let direct_samples =
        PreciseEphemerisSamples::from_samples(converted_epoch_samples(&records)).expect("source");
    let cached_samples =
        PreciseEphemerisInterpolant::from_samples(converted_epoch_samples(&records))
            .expect("sample-backed interpolant");

    for record in records {
        let parsed = sp3
            .position_at_j2000_seconds(record.sat, record.epoch.j2000_s)
            .expect("parsed state at record epoch");
        assert_record_matches(record, parsed.position.as_array(), parsed.clock_s);
        assert_record_matches_all_sample_paths(record, &direct_samples, &cached_samples);
    }
}

#[test]
fn exact_record_epochs_match_sp3_text_oracle_on_both_construction_paths() {
    let text = fixture_text(COD_5M_FIXTURE);
    let sp3 = Sp3::parse(text.as_bytes()).expect("parse SP3 fixture");
    let samples = PreciseEphemerisInterpolant::from_samples(sp3.precise_ephemeris_samples())
        .expect("sample-backed interpolant");
    let sat = gps(1);
    let records = text_records(&text, sat);
    assert!(
        records.len() >= 144,
        "fixture does not cover a 12 hour window"
    );
    assert!(
        records.iter().any(|record| record.epoch.is_boundary_node),
        "fixture does not span a 45 minute boundary node"
    );

    for &record in records.iter().take(144) {
        let parsed = sp3
            .position_at_j2000_seconds(record.sat, record.epoch.j2000_s)
            .expect("parsed state at record epoch");
        assert_record_matches(record, parsed.position.as_array(), parsed.clock_s);

        let from_samples = samples
            .position_at_j2000_seconds(record.sat, record.epoch.j2000_s)
            .expect("sample-backed state at record epoch");
        assert_record_matches(
            record,
            from_samples.position.as_array(),
            from_samples.clock_s,
        );
    }
}

#[test]
fn exact_record_epochs_match_sp3_text_oracle_on_45m_sample_backed_path() {
    let text = floor_sensitive_45m_fixture_text();
    let sp3 = Sp3::parse(text.as_bytes()).expect("parse SP3 fixture");
    let samples = PreciseEphemerisInterpolant::from_samples(sp3.precise_ephemeris_samples())
        .expect("sample-backed interpolant");
    let sat = gps(1);
    let records = text_records(&text, sat);
    assert_eq!(records.len(), 12, "fixture record coverage changed");
    for window in records.windows(2) {
        assert_eq!(
            window[1].epoch.j2000_s - window[0].epoch.j2000_s,
            2700.0,
            "fixture is no longer 45-minute cadence"
        );
    }

    for record in records {
        let parsed = sp3
            .position_at_j2000_seconds(record.sat, record.epoch.j2000_s)
            .expect("parsed state at record epoch");
        assert_record_matches(record, parsed.position.as_array(), parsed.clock_s);

        let from_samples = samples
            .position_at_j2000_seconds(record.sat, record.epoch.j2000_s)
            .expect("sample-backed state at record epoch");
        assert_record_matches(
            record,
            from_samples.position.as_array(),
            from_samples.clock_s,
        );
    }
}

#[test]
fn sample_backed_path_matches_parsed_path_bits_at_45m_midpoints() {
    let text = floor_sensitive_45m_fixture_text();
    let sp3 = Sp3::parse(text.as_bytes()).expect("parse SP3 fixture");
    let samples = PreciseEphemerisInterpolant::from_samples(sp3.precise_ephemeris_samples())
        .expect("sample-backed interpolant");
    let sat = gps(1);
    let records = text_records(&text, sat);
    assert_eq!(records.len(), 12, "fixture record coverage changed");

    for window in records.windows(2) {
        let midpoint = 0.5 * (window[0].epoch.j2000_s + window[1].epoch.j2000_s);
        let parsed = sp3
            .position_at_j2000_seconds(sat, midpoint)
            .expect("parsed midpoint state");
        let from_samples = samples
            .position_at_j2000_seconds(sat, midpoint)
            .expect("sample-backed midpoint state");
        assert_state_bits_eq(sat, midpoint, parsed, from_samples);
    }
}

#[test]
fn cached_batch_record_epochs_match_sp3_text_oracle() {
    let text = fixture_text(COD_5M_FIXTURE);
    let sp3 = Sp3::parse(text.as_bytes()).expect("parse SP3 fixture");
    let cached = PreciseEphemerisInterpolant::from_sp3(&sp3);
    let sat = gps(1);
    let records: Vec<_> = text_records(&text, sat)
        .into_iter()
        .filter(|record| record.epoch.is_boundary_node)
        .take(8)
        .collect();
    assert_eq!(records.len(), 8, "fixture boundary-node coverage changed");

    let satellites: Vec<_> = records.iter().map(|record| record.sat).collect();
    let epochs: Vec<_> = records.iter().map(|record| record.epoch.j2000_s).collect();
    let batch =
        observable_states_at_j2000_s(&cached, &satellites, &epochs).expect("batch evaluation");
    for (index, &record) in records.iter().enumerate() {
        assert_eq!(batch.element_results[index], Ok(()));
        assert_record_matches(record, batch.positions_ecef_m[index], batch.clocks_s[index]);
    }
}
