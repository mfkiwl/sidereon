//! Independent SP3 evaluation oracle.
//!
//! Fixture: `tests/fixtures/sp3/COD0MGXFIN_20201770000_01D_05M_ORB.SP3`,
//! a public IGS MGEX final orbit and clock product for 2020-06-25, committed
//! in this crate's existing SP3 fixture set. The checks below read the SP3 text
//! records directly with fixed-column parsing in the test. Expected positions
//! and clocks are not taken from `Sp3::state`, `Sp3::position_at_j2000_seconds`,
//! or the cached interpolant.

use sidereon_core::astro::time::civil::j2000_seconds;
use sidereon_core::ephemeris::{observable_states_at_j2000_s, PreciseEphemerisInterpolant, Sp3};
use sidereon_core::{GnssSatelliteId, GnssSystem};

const COD_5M_FIXTURE: &str = "tests/fixtures/sp3/COD0MGXFIN_20201770000_01D_05M_ORB.SP3";
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

#[test]
fn exact_record_epochs_match_sp3_text_oracle() {
    let text = fixture_text(COD_5M_FIXTURE);
    let sp3 = Sp3::parse(text.as_bytes()).expect("parse SP3 fixture");
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
        let state = sp3
            .position_at_j2000_seconds(record.sat, record.epoch.j2000_s)
            .expect("interpolated state at record epoch");
        assert_record_matches(record, state.position.as_array(), state.clock_s);
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
