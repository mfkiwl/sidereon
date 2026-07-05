#![cfg(sidereon_repo_tests)]

//! RINEX clock product evaluation tests.
//!
//! Real CLK/SP3 fixtures are public IGS final products mirrored in
//! `tests/fixtures/clk/IGS0OPSFIN_20261330000_90M_30S_CLK.CLK` and
//! `tests/fixtures/sp3/IGS0OPSFIN_20261330000_03H_15M_ORB.SP3`.
//! The real-product identity checks compare evaluated values against parsed
//! record rows, and compare shared CLK/SP3 record epochs to the SP3 clock field
//! resolution.

use sidereon_core::astro::time::civil::seconds_between_splits;
use sidereon_core::astro::time::model::{Instant, TimeScale};
use sidereon_core::constants::SECONDS_PER_DAY;
use sidereon_core::ephemeris::Sp3;
use sidereon_core::rinex::clock::{
    civil_to_clock_instant, civil_to_gps_seconds, ClockEpoch, RinexClock, RinexClockError,
};
use sidereon_core::{GnssSatelliteId, GnssSystem};

const CLK: &str = include_str!("fixtures/clk/synthetic_rinex_clock.clk");
const REAL_CLK: &str = include_str!("fixtures/clk/IGS0OPSFIN_20261330000_90M_30S_CLK.CLK");
const REAL_SP3: &str = include_str!("fixtures/sp3/IGS0OPSFIN_20261330000_03H_15M_ORB.SP3");

fn gps(prn: u8) -> GnssSatelliteId {
    GnssSatelliteId::new(GnssSystem::Gps, prn).expect("valid GPS PRN")
}

fn real_clk_source_rows() -> Vec<(String, Instant, f64)> {
    REAL_CLK
        .lines()
        .filter(|line| line.starts_with("AS "))
        .map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            assert!(
                fields.len() >= 10,
                "real CLK AS row has too few fields: {line}"
            );
            let year = fields[2].parse::<i32>().expect("CLK source year");
            let month = fields[3].parse::<u8>().expect("CLK source month");
            let day = fields[4].parse::<u8>().expect("CLK source day");
            let hour = fields[5].parse::<u8>().expect("CLK source hour");
            let minute = fields[6].parse::<u8>().expect("CLK source minute");
            let second = fields[7].parse::<f64>().expect("CLK source second");
            let epoch =
                civil_to_clock_instant(TimeScale::Gpst, year, month, day, hour, minute, second)
                    .expect("CLK source epoch");
            let bias_s = fields[9].parse::<f64>().expect("CLK source bias");
            (fields[1].to_string(), epoch, bias_s)
        })
        .collect()
}

#[test]
fn real_clk_record_epochs_evaluate_to_parsed_rows() {
    let clock = RinexClock::parse(REAL_CLK).expect("real RINEX clock");
    let mut checked = 0usize;

    for (satellite, epoch, bias_s) in real_clk_source_rows() {
        let evaluated = clock
            .clock_s_at_instant(&satellite, epoch)
            .expect("valid clock query")
            .expect("clock record at source epoch");
        assert_eq!(
            evaluated.to_bits(),
            bias_s.to_bits(),
            "{satellite} source-epoch clock identity"
        );
        checked += 1;
    }

    assert_eq!(checked, 5_792);
}

#[test]
fn real_clk_and_sp3_clocks_match_at_shared_record_epochs() {
    let clock = RinexClock::parse(REAL_CLK).expect("real RINEX clock");
    let sp3 = Sp3::parse(REAL_SP3.as_bytes()).expect("real SP3");
    let mut checked = 0usize;

    for epoch_index in [0usize, 6] {
        let epoch = sp3.epochs[epoch_index];
        for prn in 1..=32 {
            let satellite = gps(prn);
            let sp3_clock_s = sp3
                .state(satellite, epoch_index)
                .expect("SP3 satellite state")
                .clock_s
                .expect("SP3 clock record");
            let clk_clock_s = clock
                .clock_s_at_instant(&satellite.to_string(), epoch)
                .expect("valid CLK query")
                .expect("CLK clock at shared source epoch");
            assert!(
                (clk_clock_s - sp3_clock_s).abs() <= 5.0e-13,
                "{satellite} epoch {epoch_index} CLK {clk_clock_s:e} SP3 {sp3_clock_s:e}"
            );
            checked += 1;
        }
    }

    assert_eq!(checked, 64);
}

#[test]
fn parses_satellite_clock_records_and_ignores_receivers() {
    let clock = RinexClock::parse(CLK).expect("RINEX clock");
    assert_eq!(clock.time_scale, TimeScale::Gpst);
    let sats = clock.series.keys().cloned().collect::<Vec<_>>();
    assert_eq!(sats, vec!["G05".to_string(), "G24".to_string()]);
    assert_eq!(clock.series["G05"].len(), 3);
    assert_eq!(clock.series["G24"].len(), 2);
}

#[test]
fn exact_and_interpolated_biases_match_legacy_bits() {
    let clock = RinexClock::parse(CLK).expect("RINEX clock");

    let g05 = clock
        .clock_s("G05", epoch(2026, 5, 13, 0, 0, 30.0))
        .expect("valid clock query")
        .expect("G05 exact clock");
    assert_eq!(g05.to_bits(), 0xbf2a36e36f0d4275);

    let g24_exact = clock
        .clock_s("G24", epoch(2026, 5, 13, 0, 0, 0.0))
        .expect("valid clock query")
        .expect("G24 exact clock");
    assert_eq!(g24_exact.to_bits(), 0x3f0a36e2eb1c432d);

    let g24_mid = clock
        .clock_s("G24", epoch(2026, 5, 13, 0, 0, 15.0))
        .expect("valid clock query")
        .expect("G24 interpolated clock");
    assert_eq!(g24_mid.to_bits(), 0x3f0a36e4a2ea40ca);
}

#[test]
fn outside_span_and_unknown_satellite_have_no_clock() {
    let clock = RinexClock::parse(CLK).expect("RINEX clock");
    assert_eq!(
        clock
            .clock_s("G99", epoch(2026, 5, 13, 0, 0, 15.0))
            .expect("valid clock query"),
        None
    );
    assert_eq!(
        clock
            .clock_s("G05", epoch(2026, 5, 12, 23, 59, 0.0))
            .expect("valid clock query"),
        None
    );
    assert_eq!(
        clock
            .clock_s("G05", epoch(2026, 5, 13, 1, 0, 0.0))
            .expect("valid clock query"),
        None
    );
}

#[test]
fn duplicate_time_tags_keep_the_last_record() {
    let text = "AS G05  2026 05 13 00 00  0.000000  1   1.0e-04\n\
                AS G05  2026 05 13 00 00  0.000000  1   2.0e-04\n";
    let clock = RinexClock::parse(text).expect("RINEX clock");
    let bias = clock
        .clock_s("G05", epoch(2026, 5, 13, 0, 0, 0.0))
        .expect("valid clock query")
        .expect("duplicate point");
    assert_eq!(bias.to_bits(), (2.0e-4_f64).to_bits());
}

#[test]
fn rounded_fractional_second_carries_to_next_second() {
    let text = "AS G05  2026 05 13 00 00 59.9999996  1   1.0e-04\n";
    let clock = RinexClock::parse(text).expect("rounded clock epoch must parse");
    let expected = civil_to_gps_seconds(2026, 5, 13, 0, 1, 0.0).expect("next minute");

    assert_eq!(
        clock.series["G05"][0]
            .gps_seconds()
            .expect("GPST sample")
            .to_bits(),
        expected.to_bits()
    );
    assert_eq!(
        civil_to_gps_seconds(2026, 5, 13, 0, 0, 59.9999996)
            .expect("rounded public epoch")
            .to_bits(),
        expected.to_bits()
    );
}

#[test]
fn utc_time_system_preserves_scale_and_queries_by_utc_instant() {
    let text = " 3.00           C                                       RINEX VERSION / TYPE\n\
                UTC                                                     TIME SYSTEM ID\n\
                                                                    END OF HEADER\n\
                AS G05  2017 01 01 00 00  0.000000  1   1.0e-04\n\
                AS G05  2017 01 01 00 00 30.000000  1   2.0e-04\n";
    let clock = RinexClock::parse(text).expect("UTC RINEX clock");

    assert_eq!(clock.time_scale, TimeScale::Utc);
    assert_eq!(clock.series["G05"][0].epoch.scale, TimeScale::Utc);
    assert_eq!(clock.series_rows(), vec![("G05".to_string(), vec![])]);
    let interpolated = clock
        .clock_s("G05", epoch(2017, 1, 1, 0, 0, 15.0))
        .expect("valid clock query")
        .expect("UTC interpolated clock");
    assert!((interpolated - 1.5e-4).abs() < 1.0e-18);

    let gpst_query =
        civil_to_clock_instant(TimeScale::Gpst, 2017, 1, 1, 0, 0, 15.0).expect("GPST instant");
    assert_eq!(
        clock
            .clock_s_at_instant("G05", gpst_query)
            .expect("valid clock query"),
        None
    );

    let rows = clock.instant_series_rows();
    assert_eq!(rows[0].1[0].0.scale, TimeScale::Utc);
    let rebuilt = RinexClock::from_instant_series_rows(clock.time_scale, rows)
        .expect("valid manual RINEX clock rows");
    assert_eq!(rebuilt, clock);
}

#[test]
fn rinex_clock_utc_leap_second_interval_to_midnight_interpolates_forward() {
    let text = " 3.00           C                                       RINEX VERSION / TYPE\n\
                UTC                                                     TIME SYSTEM ID\n\
                                                                    END OF HEADER\n\
                AS G05  2016 12 31 23 59 60.250000  1   1.0e-04\n\
                AS G05  2017 01 01 00 00  0.000000  1   4.0e-04\n";
    let clock = RinexClock::parse(text).expect("UTC leap-second RINEX clock");
    let points = &clock.series["G05"];
    assert_eq!(points.len(), 2);

    let leap = points[0].epoch.julian_date().expect("leap-second split");
    let midnight = points[1].epoch.julian_date().expect("midnight split");
    assert_eq!(leap.jd_whole.to_bits(), midnight.jd_whole.to_bits());
    assert!(leap.fraction < midnight.fraction);
    let span_s = seconds_between_splits(
        midnight.jd_whole,
        midnight.fraction,
        leap.jd_whole,
        leap.fraction,
    );
    assert!((span_s - 0.75).abs() < 1.0e-12);

    let interpolated = clock
        .clock_s("G05", epoch(2016, 12, 31, 23, 59, 60.625))
        .expect("valid clock query")
        .expect("leap-second interpolation");
    assert!((interpolated - 2.5e-4).abs() < 1.0e-18);
}

#[test]
fn rejects_gps_time_leap_second_label() {
    let text = "AS G05  2016 12 31 23 59 60.000000  1   1.0e-04\n";
    let err = RinexClock::parse(text).expect_err("GPS-time clock leap second must error");
    assert_eq!(
        err,
        RinexClockError::BadField {
            line: 1,
            field: "epoch",
            value: "2016 12 31 23 59 60".to_string(),
        }
    );
    assert_eq!(civil_to_gps_seconds(2016, 12, 31, 23, 59, 60.0), None);
}

#[test]
fn strict_parse_reports_short_as_records() {
    let text = "AS G05  2026 05 13 00 00  0.000000  1\n";
    let err = RinexClock::parse(text).expect_err("short AS record must error");
    assert_eq!(
        err,
        RinexClockError::MalformedAsRecord {
            line: 1,
            reason: "expected at least 10 fields",
            record: "AS G05  2026 05 13 00 00  0.000000  1".to_string(),
        }
    );
}

#[test]
fn strict_parse_reports_bad_as_fields() {
    let text = "AS G05  2026 05 13 00 00  bad-second  1   1.0e-04\n";
    let err = RinexClock::parse(text).expect_err("bad AS field must error");
    assert_eq!(
        err,
        RinexClockError::BadField {
            line: 1,
            field: "second",
            value: "bad-second".to_string(),
        }
    );
}

#[test]
fn strict_parse_rejects_malformed_fractional_second() {
    let text = "AS G05  2026 05 13 00 00  59.  1   1.0e-04\n";
    let err = RinexClock::parse(text).expect_err("malformed AS fraction must error");
    assert_eq!(
        err,
        RinexClockError::BadField {
            line: 1,
            field: "second",
            value: "59.".to_string(),
        }
    );
}

#[test]
fn strict_parse_rejects_invalid_leap_second_range() {
    for second in ["61.000000", "-1.000000"] {
        let text = format!("AS G05  2016 12 31 23 59 {second:>10}  1   1.0e-04\n");
        let err = RinexClock::parse(&text).expect_err("invalid AS second must error");
        assert_eq!(
            err,
            RinexClockError::BadField {
                line: 1,
                field: "epoch",
                value: format!("2016 12 31 23 59 {}", second.parse::<f64>().unwrap()),
            }
        );
    }
}

#[test]
fn strict_parse_rejects_invalid_civil_date() {
    let text = "AS G05  2026 13 31 23 59  0.000000  1   1.0e-04\n";
    let err = RinexClock::parse(text).expect_err("invalid AS date must error");
    assert_eq!(
        err,
        RinexClockError::BadField {
            line: 1,
            field: "epoch",
            value: "2026 13 31 23 59 0".to_string(),
        }
    );
}

#[test]
fn parse_lossy_keeps_legacy_skip_behavior() {
    let text = "AS G05  2026 05 13 00 00  0.000000  1   1.0e-04\n\
                AS G06  2026 05 13 00 00  bad-second  1   2.0e-04\n";
    let clock = RinexClock::parse_lossy(text);
    assert_eq!(
        clock.series.keys().cloned().collect::<Vec<_>>(),
        vec!["G05"]
    );
    assert_eq!(
        clock
            .clock_s("G05", epoch(2026, 5, 13, 0, 0, 0.0))
            .expect("valid clock query")
            .expect("G05 clock")
            .to_bits(),
        (1.0e-4_f64).to_bits()
    );
}

#[test]
fn civil_gps_seconds_match_gps_epoch_boundary() {
    assert_eq!(
        civil_to_gps_seconds(1980, 1, 6, 0, 0, 0.0).expect("GPS epoch"),
        0.0
    );
    assert_eq!(
        civil_to_gps_seconds(1980, 1, 7, 0, 0, 0.0).expect("next day"),
        SECONDS_PER_DAY
    );
}

fn epoch(year: i32, month: u8, day: u8, hour: u8, minute: u8, second: f64) -> ClockEpoch {
    ClockEpoch {
        year,
        month,
        day,
        hour,
        minute,
        second,
    }
}
