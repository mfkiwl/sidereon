//! Focused first-slice RINEX QC tests.
//!
//! Broken inputs are deterministic mutations of small in-test products or
//! existing public fixtures under `tests/fixtures`. The QC layer delegates text
//! decoding to the existing RINEX and CRINEX modules; these tests cover the
//! typed rule/action surface added by this module.

use super::*;

fn header_line(body: &str, label: &str) -> String {
    format!("{body:<60}{label}")
}

fn obs_text(headers: &[String], body: &str) -> String {
    let mut lines = vec![
        header_line(
            "     3.05           OBSERVATION DATA    M (MIXED)",
            "RINEX VERSION / TYPE",
        ),
        header_line("QC01", "MARKER NAME"),
        header_line(
            "        0.0000        0.0000        0.0000",
            "ANTENNA: DELTA H/E/N",
        ),
        header_line("G    1 C1C", "SYS / # / OBS TYPES"),
    ];
    lines.extend(headers.iter().cloned());
    lines.push(header_line("", "END OF HEADER"));
    lines.extend(body.lines().map(str::to_string));
    lines.join("\n")
}

fn gps_epoch(minute: u8, second: f64, value: &str) -> String {
    format!("> 2020 01 01 00 {minute:02}{second:11.7}  0  1\nG01{value}")
}

fn finding_codes(report: &LintReport) -> Vec<&'static str> {
    report.findings.iter().map(Finding::code).collect()
}

fn error_or_fatal_codes(report: &LintReport) -> Vec<&'static str> {
    report
        .findings
        .iter()
        .filter(|finding| matches!(finding.severity(), Severity::Fatal | Severity::Error))
        .map(Finding::code)
        .collect()
}

fn nav_fixture() -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/nav/ESBC00DNK_R_20201770000_01D_MN.rnx"
    );
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read NAV fixture {path}: {e}"))
}

#[test]
fn real_rinex3_obs_fixtures_have_no_error_or_fatal_findings() {
    let fixtures = [
        "ESBC00DNK_R_20201770000_01D_30S_MO_120epoch.rnx",
        "PASA00ESP_R_20261201000_02H_30S_MO.rnx",
        "SCOA00FRA_R_20261201000_02H_30S_MO.rnx",
        "WTZZ00DEU_R_20201770000_01D_30S_MO_120epoch.rnx",
    ];
    for fixture in fixtures {
        let path = format!(
            "{}/tests/fixtures/obs/{fixture}",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let report = lint_obs_text(&text);
        let bad = error_or_fatal_codes(&report);
        assert!(bad.is_empty(), "{fixture}: {bad:?} {:?}", report.findings);
    }
}

#[test]
fn obs_code_tables_accept_fixture_deltas_and_reject_bad_bands() {
    let accepted = [
        (GnssSystem::Glonass, "C3Q"),
        (GnssSystem::Glonass, "D3Q"),
        (GnssSystem::Glonass, "L3Q"),
        (GnssSystem::Glonass, "S3Q"),
        (GnssSystem::Qzss, "C5Q"),
        (GnssSystem::Qzss, "D5Q"),
        (GnssSystem::Qzss, "L5Q"),
        (GnssSystem::Qzss, "S5Q"),
        (GnssSystem::Gps, "C1N"),
        (GnssSystem::Sbas, "C5Q"),
        (GnssSystem::BeiDou, "C7Z"),
    ];
    for (system, code) in accepted {
        assert!(is_valid_obs_code(system, code, 3.05), "{system:?} {code}");
    }
    assert!(!is_valid_obs_code(GnssSystem::Gps, "C9C", 3.05));
}

#[test]
fn obs_lint_reports_time_interval_order_and_repair_fixes_them() {
    let headers = [
        header_line(
            "  2020    01    01    00    00    1.0000000     GPS",
            "TIME OF FIRST OBS",
        ),
        header_line("    60.000", "INTERVAL"),
    ];
    let body = [
        gps_epoch(1, 0.0, "  20000000.000  "),
        gps_epoch(0, 0.0, "  21000000.000  "),
        gps_epoch(0, 30.0, "  20000000.000  "),
        gps_epoch(0, 0.0, "  22000000.000  "),
    ]
    .join("\n");
    let obs = RinexObs::parse(&obs_text(&headers, &body)).expect("parse OBS");

    let report = lint_obs(&obs);
    let codes = finding_codes(&report);
    assert!(codes.contains(&"OBS-H07"), "{codes:?}");
    assert!(codes.contains(&"OBS-H09"), "{codes:?}");
    assert!(codes.contains(&"OBS-B01"), "{codes:?}");
    assert!(codes.contains(&"OBS-B02"), "{codes:?}");
    assert!(!report.is_clean());

    let repair = repair_obs(
        &obs,
        &RepairOptions {
            set_interval: true,
            ..RepairOptions::default()
        },
    );
    let actions: Vec<_> = repair.actions.iter().map(|action| action.id).collect();
    assert!(actions.contains(&"A3"), "{actions:?}");
    assert!(actions.contains(&"A4"), "{actions:?}");
    assert!(actions.contains(&"A6"), "{actions:?}");
    let remaining = finding_codes(&repair.remaining);
    assert!(!remaining.contains(&"OBS-H07"), "{remaining:?}");
    assert!(!remaining.contains(&"OBS-H09"), "{remaining:?}");
    assert!(!remaining.contains(&"OBS-B01"), "{remaining:?}");
    assert!(!remaining.contains(&"OBS-B02"), "{remaining:?}");

    let second = repair_obs(&repair.repaired, &RepairOptions::default());
    assert!(second.actions.is_empty(), "{:?}", second.actions);
}

#[test]
fn obs_drop_empty_satellite_record_is_opt_in() {
    let headers = [header_line(
        "  2020    01    01    00    00    0.0000000     GPS",
        "TIME OF FIRST OBS",
    )];
    let body = "> 2020 01 01 00 00  0.0000000  0  1\nG01";
    let obs = RinexObs::parse(&obs_text(&headers, body)).expect("parse OBS");

    let report = lint_obs(&obs);
    assert!(finding_codes(&report).contains(&"OBS-B08"));

    let repair = repair_obs(
        &obs,
        &RepairOptions {
            drop_empty_records: true,
            ..RepairOptions::default()
        },
    );
    assert_eq!(
        repair
            .actions
            .iter()
            .map(|action| action.id)
            .collect::<Vec<_>>(),
        vec!["A7"]
    );
    assert!(!finding_codes(&repair.remaining).contains(&"OBS-B08"));
}

#[test]
fn obs_repair_duplicate_epoch_keeps_first_satellite_row() {
    let headers = [header_line(
        "  2020    01    01    00    00    0.0000000     GPS",
        "TIME OF FIRST OBS",
    )];
    let body = [
        gps_epoch(0, 0.0, "  11111111.000  "),
        gps_epoch(0, 0.0, "  22222222.000  "),
    ]
    .join("\n");
    let obs = RinexObs::parse(&obs_text(&headers, &body)).expect("parse OBS");
    let repair = repair_obs(&obs, &RepairOptions::default());
    assert_eq!(repair.repaired.epochs.len(), 1);
    let g01 = GnssSatelliteId::new(GnssSystem::Gps, 1).expect("G01");
    assert_eq!(
        repair.repaired.epochs[0].sats[&g01][0].value,
        Some(11_111_111.0)
    );
    assert!(repair.actions[0].message.contains("G01"));
}

#[test]
fn glonass_slot_findings_are_per_satellite_and_check_channel_range() {
    let headers = [
        header_line(
            "  2020    01    01    00    00    0.0000000     GPS",
            "TIME OF FIRST OBS",
        ),
        header_line("R    1 C1C", "SYS / # / OBS TYPES"),
    ];
    let body = [
        "> 2020 01 01 00 00  0.0000000  0  1\nR01  20000000.000  ",
        "> 2020 01 01 00 00 30.0000000  0  1\nR01  20000001.000  ",
    ]
    .join("\n");
    let mut obs = RinexObs::parse(&obs_text(&headers, &body)).expect("parse OBS");
    let report = lint_obs(&obs);
    let h12 = report
        .findings
        .iter()
        .filter(|finding| finding.code() == "OBS-H12")
        .count();
    assert_eq!(h12, 1, "{:?}", report.findings);

    obs.header.glonass_slots.insert(1, 99);
    let report = lint_obs(&obs);
    assert!(report.findings.iter().any(|finding| {
        matches!(
            finding,
            Finding::ObsGlonassSlotIssue {
                issue: "invalid channel",
                ..
            }
        )
    }));
}

#[test]
fn obs_text_repair_guards_event_special_records() {
    let headers = [header_line(
        "  2020    01    01    00    00    0.0000000     GPS",
        "TIME OF FIRST OBS",
    )];
    let body = "> 2020 01 01 00 00  0.0000000  2  1\nCOMMENT";
    let text = obs_text(&headers, body);
    assert!(repair_obs_text(&text, &RepairOptions::default()).is_err());

    let repair = repair_obs_text(
        &text,
        &RepairOptions {
            drop_unsupported: true,
            ..RepairOptions::default()
        },
    )
    .expect("drop unsupported event records");
    assert!(repair.actions.iter().any(|action| action.id == "OBS-B11"));
}

#[test]
fn obs_text_repair_guards_unretained_header_records() {
    let headers = [header_line("payload", "UNSUPPORTED LABEL")];
    let text = obs_text(&headers, &gps_epoch(0, 0.0, "  20000000.000  "));
    let report = lint_obs_text(&text);
    assert!(finding_codes(&report).contains(&"OBS-H90"));
    assert!(repair_obs_text(&text, &RepairOptions::default()).is_err());

    let repair = repair_obs_text(
        &text,
        &RepairOptions {
            drop_unsupported: true,
            ..RepairOptions::default()
        },
    )
    .expect("drop unsupported header");
    assert!(repair.actions.iter().any(|action| action.id == "OBS-H90"));
    assert!(!finding_codes(&repair.remaining).contains(&"OBS-H90"));
}

#[test]
fn rinex4_epoch_extension_and_clock_offset_round_trip() {
    let text = [
        header_line(
            "     4.02           OBSERVATION DATA    M (MIXED)",
            "RINEX VERSION / TYPE",
        ),
        header_line("G    1 C1C", "SYS / # / OBS TYPES"),
        header_line("", "END OF HEADER"),
        "> 2026 01 02 03 04  5.0000000 12345  0  1  0.123456789012".to_string(),
        "G01  20000000.000  ".to_string(),
    ]
    .join("\n");
    let obs = RinexObs::parse(&text).expect("parse RINEX 4 OBS");
    assert_eq!(obs.epochs[0].epoch_picoseconds, Some(12345));
    assert_eq!(obs.epochs[0].rcv_clock_offset_s, Some(0.123456789012));
    let reparsed = RinexObs::parse(&obs.to_rinex_string()).expect("reparse RINEX 4 OBS");
    assert_eq!(reparsed, obs);
}

#[test]
fn obs_text_lint_decodes_crinex_before_linting() {
    let rnx_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/obs/ESBC00DNK_R_20201770000_01D_30S_MO_trim.rnx"
    );
    let crx_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/obs/ESBC00DNK_R_20201770000_01D_30S_MO_trim.crx"
    );
    let rnx = std::fs::read_to_string(rnx_path).expect("read RINEX fixture");
    let crx = std::fs::read_to_string(crx_path).expect("read CRINEX fixture");

    let rnx_report = lint_obs_text(&rnx);
    let crx_report = lint_obs_text(&crx);
    assert!(!rnx_report.decoded_from_crinex);
    assert!(crx_report.decoded_from_crinex);
    assert_eq!(finding_codes(&crx_report), finding_codes(&rnx_report));
}

#[test]
fn nav_lint_and_repair_identical_duplicates_and_order() {
    let records = parse_nav(&nav_fixture()).expect("parse NAV fixture");
    assert!(records.len() >= 2);
    let mut damaged = vec![records[1], records[0], records[0]];

    let report = LintReport {
        findings: nav_findings(&damaged),
        decoded_from_crinex: false,
    };
    let codes = finding_codes(&report);
    assert!(codes.contains(&"NAV-B02"), "{codes:?}");
    assert!(codes.contains(&"NAV-B03"), "{codes:?}");

    let repair = repair_nav(&damaged, &RepairOptions::default());
    let actions: Vec<_> = repair.actions.iter().map(|action| action.id).collect();
    assert!(actions.contains(&"A11"), "{actions:?}");
    assert!(actions.contains(&"A12"), "{actions:?}");
    damaged.sort_by_key(nav_sort_key);
    assert_eq!(repair.records.len(), 2);
    assert!(!finding_codes(&repair.remaining).contains(&"NAV-B02"));
    assert!(!finding_codes(&repair.remaining).contains(&"NAV-B03"));
}

#[test]
fn nav_text_lint_reports_header_findings_without_file_io() {
    let text = nav_fixture().replace(
        "    18                                                      LEAP SECONDS",
        "",
    );
    let report = lint_nav_text(&text);
    assert!(finding_codes(&report).contains(&"NAV-H02"));
}

#[test]
fn nav_text_repair_guards_out_of_scope_records() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/nav/ESBC00DNK_R_20201770000_01D_RN.rnx"
    );
    let text = std::fs::read_to_string(path).expect("read GLONASS NAV fixture");
    assert!(repair_nav_text(&text, &RepairOptions::default()).is_err());

    let repair = repair_nav_text(
        &text,
        &RepairOptions {
            drop_unsupported: true,
            ..RepairOptions::default()
        },
    )
    .expect("drop unsupported NAV records");
    assert!(repair.actions.iter().any(|action| action.id == "NAV-B06"));
}
